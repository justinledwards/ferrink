use std::io::Read;
use std::path::Path;

use ferrink_platform::{ArmFloatAbi, ElfAbi, ElfClass, Endianness};

const ELF_MACHINE_ARM: u16 = 40;
const ARM_FLOAT_SOFT: u32 = 0x200;
const ARM_FLOAT_HARD: u32 = 0x400;

pub(crate) fn read_elf_abi(path: &Path) -> std::io::Result<Option<ElfAbi>> {
    let mut file = std::fs::File::open(path)?;
    let mut header = [0u8; 64];
    let length = file.read(&mut header)?;
    Ok(parse_elf_abi(&header[..length]))
}

pub(crate) fn parse_elf_abi(header: &[u8]) -> Option<ElfAbi> {
    if header.len() < 52 || header.get(..4)? != b"\x7fELF" {
        return None;
    }
    let class = match header[4] {
        1 => ElfClass::Elf32,
        2 if header.len() >= 64 => ElfClass::Elf64,
        _ => return None,
    };
    let endianness = match header[5] {
        1 => Endianness::Little,
        2 => Endianness::Big,
        _ => return None,
    };
    let read_u16 = |offset: usize| -> Option<u16> {
        let bytes: [u8; 2] = header.get(offset..offset + 2)?.try_into().ok()?;
        Some(match endianness {
            Endianness::Little => u16::from_le_bytes(bytes),
            Endianness::Big => u16::from_be_bytes(bytes),
        })
    };
    let read_u32 = |offset: usize| -> Option<u32> {
        let bytes: [u8; 4] = header.get(offset..offset + 4)?.try_into().ok()?;
        Some(match endianness {
            Endianness::Little => u32::from_le_bytes(bytes),
            Endianness::Big => u32::from_be_bytes(bytes),
        })
    };
    let machine = read_u16(18)?;
    let flags = read_u32(match class {
        ElfClass::Elf32 => 36,
        ElfClass::Elf64 => 48,
    })?;
    let (arm_eabi_version, arm_float_abi) = if machine == ELF_MACHINE_ARM {
        let float_abi = match (flags & ARM_FLOAT_HARD != 0, flags & ARM_FLOAT_SOFT != 0) {
            (true, false) => ArmFloatAbi::Hard,
            (false, true) => ArmFloatAbi::Soft,
            (false, false) => ArmFloatAbi::Unspecified,
            (true, true) => ArmFloatAbi::ConflictingFlags,
        };
        (Some((flags >> 24) as u8), Some(float_abi))
    } else {
        (None, None)
    };
    Some(ElfAbi {
        class,
        endianness,
        machine,
        flags,
        arm_eabi_version,
        arm_float_abi,
    })
}

#[cfg(test)]
mod tests {
    use ferrink_platform::ArmFloatAbi;

    use super::parse_elf_abi;

    #[test]
    fn parses_arm_eabi5_hard_float_flags() {
        let mut header = [0u8; 52];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 1;
        header[5] = 1;
        header[18..20].copy_from_slice(&40u16.to_le_bytes());
        header[36..40].copy_from_slice(&0x0500_0400u32.to_le_bytes());
        let abi = parse_elf_abi(&header).unwrap();
        assert_eq!(abi.arm_eabi_version, Some(5));
        assert_eq!(abi.arm_float_abi, Some(ArmFloatAbi::Hard));
    }

    #[test]
    fn malformed_headers_are_rejected() {
        assert!(parse_elf_abi(b"not an elf").is_none());
    }
}
