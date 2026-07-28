use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("Cargo did not provide OUT_DIR"))?;
    let asset_dir = out_dir.join("ferrink-assets");
    let font_path = asset_dir.join("InterVariable.ttf");

    std::fs::create_dir_all(&asset_dir)?;
    std::fs::write(&font_path, damascene_fonts_inter::INTER_VARIABLE)?;
    println!(
        "cargo:rustc-env=FERRINK_BUILD_SHELL_FONT_PATH={}",
        font_path.display()
    );
    println!("cargo:rerun-if-changed=build.rs");

    slint_build::compile("ui/shell.slint")?;
    Ok(())
}
