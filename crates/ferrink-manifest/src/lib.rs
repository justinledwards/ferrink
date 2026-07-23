//! Toolkit-neutral Ferrink application manifest schema and registration catalog.

#![deny(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The only manifest schema accepted by this release.
pub const MANIFEST_VERSION: u32 = 1;
/// Maximum encoded manifest size accepted before TOML parsing.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Maximum number of applications accepted by one catalog or transport.
pub const MAX_APPLICATIONS: usize = 64;
const MAX_ID_BYTES: usize = 128;
const MAX_NAME_CHARS: usize = 80;
const MAX_DESCRIPTION_CHARS: usize = 160;
const MAX_ICON_PATH_BYTES: usize = 4 * 1024;
const MAX_LICENSE_BYTES: usize = 128;
const MAX_COMMAND_ARGUMENTS: usize = 32;
const MAX_ARGUMENT_BYTES: usize = 4 * 1024;
const MAX_ENVIRONMENT_ENTRIES: usize = 8;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 4 * 1024;
const MAX_STOCK_SERVICES: usize = 32;
const MAX_STARTUP_TIMEOUT_SECONDS: u32 = 300;
const MAX_EXIT_TIMEOUT_SECONDS: u32 = 60;

/// Parsed but not yet semantically validated application data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationManifest {
    /// Version of this data contract.
    pub manifest_version: u32,
    /// Stable reverse-domain application identifier.
    pub id: String,
    /// Short user-facing application name.
    pub name: String,
    /// One-line user-facing purpose shown in the launcher.
    pub description: String,
    /// Absolute path to a package-owned PNG launcher icon.
    pub icon: String,
    /// SPDX license expression for the application package.
    pub spdx_license: String,
    /// Exact executable and argument vector. No shell parsing occurs.
    pub command: Vec<String>,
    /// Presentation ownership requested by the application.
    pub display: DisplayRequirements,
    /// Runtime capabilities required before the application is shown.
    pub requirements: RuntimeRequirements,
    /// Bounded child lifecycle policy.
    pub lifecycle: LifecyclePolicy,
    /// Explicit, restricted environment passed after clearing inherited state.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// Application presentation class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// Direct framebuffer and input ownership.
    Framebuffer,
    /// The vendor stock interface as a compatibility application.
    Stock,
    /// No interactive presentation ownership.
    Background,
    /// Application presented through the vendor X11 stack.
    X11,
}

/// Display portion of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayRequirements {
    /// Required presentation class.
    pub mode: DisplayMode,
    /// Which side owns the platform foreground transition around the child.
    pub handoff: DisplayHandoff,
}

/// Foreground transition used to start and stop an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayHandoff {
    /// Ferrink retains its foreground lease while the application runs.
    Supervisor,
    /// Ferrink restores stock before an application with its own stock wrapper.
    StockMediated,
}

/// Capability portion of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRequirements {
    /// Whether a usable Wi-Fi connection is required at launch.
    pub wifi: bool,
    /// Vendor services that must remain available while the child runs.
    pub stock_services: Vec<String>,
    /// Whether the supervisor must inhibit automatic suspend.
    pub prevent_suspend: bool,
}

/// Child lifecycle portion of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecyclePolicy {
    /// Maximum time for the child to report readiness.
    pub startup_timeout_seconds: u32,
    /// Maximum graceful-exit interval before escalation.
    pub exit_timeout_seconds: u32,
    /// Restart behavior after child exit.
    pub restart: RestartPolicy,
}

/// Restart policy for one managed application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Return to Ferrink after every exit.
    Never,
    /// Restart only after an unsuccessful exit.
    OnFailure,
    /// Restart after every exit, subject to the supervisor crash budget.
    Always,
}

/// Application data that passed every schema-v1 semantic invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedApplicationManifest(ApplicationManifest);

impl ValidatedApplicationManifest {
    /// Parses a bounded TOML manifest and validates all semantic invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] for oversized, malformed, unsupported, or
    /// semantically invalid input.
    pub fn from_toml(input: &str) -> Result<Self, ManifestError> {
        if input.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ManifestError::InputTooLarge);
        }
        let manifest: ApplicationManifest = toml::from_str(input).map_err(ManifestError::Toml)?;
        Self::try_from(manifest)
    }

    /// Reads one regular, non-symlink manifest file with a pre-parse size bound.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] if the path is not a bounded regular file or
    /// if its UTF-8 TOML data is invalid.
    pub fn from_path(path: &Path) -> Result<Self, ManifestError> {
        let metadata = path.symlink_metadata().map_err(ManifestError::Io)?;
        if !metadata.file_type().is_file() {
            return Err(ManifestError::NotRegularFile(path.to_path_buf()));
        }
        if metadata.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::InputTooLarge);
        }
        let mut input = String::new();
        File::open(path)
            .map_err(ManifestError::Io)?
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_string(&mut input)
            .map_err(ManifestError::Io)?;
        if input.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ManifestError::InputTooLarge);
        }
        Self::from_toml(&input)
    }

    /// Returns the validated application data.
    #[must_use]
    pub const fn manifest(&self) -> &ApplicationManifest {
        &self.0
    }

    /// Consumes the validation wrapper.
    #[must_use]
    pub fn into_manifest(self) -> ApplicationManifest {
        self.0
    }
}

impl TryFrom<ApplicationManifest> for ValidatedApplicationManifest {
    type Error = ManifestError;

    fn try_from(manifest: ApplicationManifest) -> Result<Self, Self::Error> {
        let mut violations = Vec::new();
        validate_manifest(&manifest, &mut violations);
        if violations.is_empty() {
            Ok(Self(manifest))
        } else {
            Err(ManifestError::Validation(violations))
        }
    }
}

/// One deterministic in-memory registry of validated application manifests.
#[derive(Debug, Default)]
pub struct ApplicationCatalog {
    applications: BTreeMap<String, ValidatedApplicationManifest>,
}

impl ApplicationCatalog {
    /// Registers one validated application by stable ID.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] on a duplicate ID or when the catalog limit is
    /// already reached.
    pub fn register(
        &mut self,
        application: ValidatedApplicationManifest,
    ) -> Result<(), CatalogError> {
        if self.applications.len() >= MAX_APPLICATIONS {
            return Err(CatalogError::TooManyApplications);
        }
        let id = application.manifest().id.clone();
        if self.applications.contains_key(&id) {
            return Err(CatalogError::DuplicateId(id));
        }
        self.applications.insert(id, application);
        Ok(())
    }

    /// Looks up one application by exact stable ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ValidatedApplicationManifest> {
        self.applications.get(id)
    }

    /// Iterates in stable identifier order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ValidatedApplicationManifest> {
        self.applications.values()
    }

    /// Returns the number of registered applications.
    #[must_use]
    pub fn len(&self) -> usize {
        self.applications.len()
    }

    /// Returns whether no applications are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.applications.is_empty()
    }
}

fn validate_manifest(manifest: &ApplicationManifest, violations: &mut Vec<ManifestViolation>) {
    if manifest.manifest_version != MANIFEST_VERSION {
        violation(
            violations,
            "manifest_version",
            format!("must equal {MANIFEST_VERSION}"),
        );
    }
    if !valid_application_id(&manifest.id) {
        violation(
            violations,
            "id",
            "must be a bounded lower-case reverse-domain identifier",
        );
    }
    if !valid_display_text(&manifest.name, MAX_NAME_CHARS) {
        violation(violations, "name", "must be trimmed, bounded display text");
    }
    if !valid_display_text(&manifest.description, MAX_DESCRIPTION_CHARS) {
        violation(
            violations,
            "description",
            "must be trimmed, bounded display text",
        );
    }
    if manifest.icon.len() > MAX_ICON_PATH_BYTES
        || !clean_absolute_path(Path::new(&manifest.icon))
        || !manifest.icon.ends_with(".png")
    {
        violation(
            violations,
            "icon",
            "must be a bounded clean absolute PNG path",
        );
    }
    if !valid_spdx_expression(&manifest.spdx_license) {
        violation(
            violations,
            "spdx_license",
            "must be a bounded ASCII SPDX expression",
        );
    }
    validate_command(&manifest.command, violations);
    validate_requirements(&manifest.requirements, violations);
    validate_lifecycle(&manifest.lifecycle, violations);
    validate_environment(&manifest.environment, violations);
}

fn valid_application_id(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_ID_BYTES {
        return false;
    }
    let mut segments = value.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let mut count = 1;
    if !valid_id_segment(first) {
        return false;
    }
    for segment in segments {
        count += 1;
        if !valid_id_segment(segment) {
            return false;
        }
    }
    count >= 2
}

fn valid_id_segment(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn valid_display_text(value: &str, maximum_chars: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= maximum_chars
        && !value.chars().any(char::is_control)
}

fn valid_spdx_expression(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LICENSE_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+' | b'(' | b')' | b' ')
        })
}

fn validate_command(command: &[String], violations: &mut Vec<ManifestViolation>) {
    if command.is_empty() || command.len() > MAX_COMMAND_ARGUMENTS {
        violation(
            violations,
            "command",
            "must contain between 1 and 32 argv entries",
        );
        return;
    }
    for argument in command {
        if argument.is_empty()
            || argument.len() > MAX_ARGUMENT_BYTES
            || argument.chars().any(char::is_control)
        {
            violation(
                violations,
                "command",
                "argv entries must be non-empty, bounded, and control-free",
            );
            break;
        }
    }
    let executable = Path::new(&command[0]);
    if !clean_absolute_path(executable) {
        violation(
            violations,
            "command[0]",
            "executable must be a clean absolute path",
        );
    }
}

fn clean_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn validate_requirements(
    requirements: &RuntimeRequirements,
    violations: &mut Vec<ManifestViolation>,
) {
    if requirements.stock_services.len() > MAX_STOCK_SERVICES {
        violation(
            violations,
            "requirements.stock_services",
            "contains too many services",
        );
    }
    let mut unique = BTreeSet::new();
    for service in &requirements.stock_services {
        let valid = !service.is_empty()
            && service.len() <= 128
            && service
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if !valid || !unique.insert(service.as_str()) {
            violation(
                violations,
                "requirements.stock_services",
                "services must be unique bounded identifiers",
            );
            break;
        }
    }
}

fn validate_lifecycle(lifecycle: &LifecyclePolicy, violations: &mut Vec<ManifestViolation>) {
    if !(1..=MAX_STARTUP_TIMEOUT_SECONDS).contains(&lifecycle.startup_timeout_seconds) {
        violation(
            violations,
            "lifecycle.startup_timeout_seconds",
            "must be between 1 and 300",
        );
    }
    if !(1..=MAX_EXIT_TIMEOUT_SECONDS).contains(&lifecycle.exit_timeout_seconds) {
        violation(
            violations,
            "lifecycle.exit_timeout_seconds",
            "must be between 1 and 60",
        );
    }
}

fn validate_environment(
    environment: &BTreeMap<String, String>,
    violations: &mut Vec<ManifestViolation>,
) {
    if environment.len() > MAX_ENVIRONMENT_ENTRIES {
        violation(violations, "environment", "contains too many entries");
    }
    for (name, value) in environment {
        if !matches!(
            name.as_str(),
            "PATH" | "HOME" | "TMPDIR" | "LANG" | "LC_ALL" | "TZ"
        ) {
            violation(
                violations,
                "environment",
                format!("variable {name:?} is not allowlisted"),
            );
            continue;
        }
        if value.is_empty()
            || value.len() > MAX_ENVIRONMENT_VALUE_BYTES
            || value.chars().any(char::is_control)
        {
            violation(
                violations,
                "environment",
                format!("variable {name:?} has an invalid value"),
            );
            continue;
        }
        if name == "PATH"
            && !value
                .split(':')
                .all(|entry| clean_absolute_path(Path::new(entry)))
        {
            violation(
                violations,
                "environment.PATH",
                "every PATH entry must be a clean absolute path",
            );
        }
        if matches!(name.as_str(), "HOME" | "TMPDIR") && !clean_absolute_path(Path::new(value)) {
            violation(
                violations,
                "environment",
                format!("variable {name:?} must be a clean absolute path"),
            );
        }
    }
}

fn violation(
    violations: &mut Vec<ManifestViolation>,
    field: &'static str,
    message: impl Into<String>,
) {
    violations.push(ManifestViolation {
        field,
        message: message.into(),
    });
}

/// One semantic manifest validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestViolation {
    /// Stable field path.
    pub field: &'static str,
    /// Human-readable invariant failure without application data.
    pub message: String,
}

/// Manifest loading or validation failure.
#[derive(Debug)]
pub enum ManifestError {
    /// Input exceeded [`MAX_MANIFEST_BYTES`].
    InputTooLarge,
    /// Input was not valid schema-v1 TOML.
    Toml(toml::de::Error),
    /// Input failed one or more semantic invariants.
    Validation(Vec<ManifestViolation>),
    /// Manifest file could not be inspected or read.
    Io(std::io::Error),
    /// Manifest path was not a regular non-symlink file.
    NotRegularFile(PathBuf),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge => formatter.write_str("application manifest is too large"),
            Self::Toml(error) => write!(formatter, "invalid application manifest TOML: {error}"),
            Self::Validation(violations) => write!(
                formatter,
                "application manifest failed {} validation rule(s)",
                violations.len()
            ),
            Self::Io(error) => write!(formatter, "cannot read application manifest: {error}"),
            Self::NotRegularFile(path) => {
                write!(
                    formatter,
                    "application manifest is not a regular file: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toml(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InputTooLarge | Self::Validation(_) | Self::NotRegularFile(_) => None,
        }
    }
}

/// Application catalog registration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// Another application already uses this stable ID.
    DuplicateId(String),
    /// The bounded catalog is full.
    TooManyApplications,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(formatter, "duplicate application ID: {id}"),
            Self::TooManyApplications => formatter.write_str("application catalog is full"),
        }
    }
}

impl std::error::Error for CatalogError {}

#[cfg(test)]
mod tests {
    use super::*;

    const KOREADER: &str = r#"
manifest_version = 1
id = "org.koreader.reader"
name = "KOReader"
description = "Open your library and continue reading"
icon = "/mnt/us/ferrink/apps/org.koreader.reader/icon.png"
spdx_license = "AGPL-3.0-only"
command = ["/mnt/us/ferrink/apps/org.koreader.reader/launch-kindle.sh"]

[display]
mode = "framebuffer"
handoff = "supervisor"

[requirements]
wifi = false
stock_services = ["com.lab126.powerd"]
prevent_suspend = false

[lifecycle]
startup_timeout_seconds = 20
exit_timeout_seconds = 10
restart = "never"

[environment]
PATH = "/usr/sbin:/usr/bin:/sbin:/bin"
"#;

    const HOME_ASSISTANT: &str = r#"
manifest_version = 1
id = "io.home-assistant.dashboard"
name = "Home Assistant"
description = "Control your home from the Kindle dashboard"
icon = "/mnt/us/ferrink/apps/io.home-assistant.dashboard/icon.png"
spdx_license = "GPL-3.0-only"
command = ["/mnt/us/ferrink/apps/io.home-assistant.dashboard/launch-kindle.sh"]

[display]
mode = "framebuffer"
handoff = "supervisor"

[requirements]
wifi = true
stock_services = ["com.lab126.powerd", "com.lab126.wifid"]
prevent_suspend = true

[lifecycle]
startup_timeout_seconds = 20
exit_timeout_seconds = 10
restart = "never"

[environment]
PATH = "/usr/sbin:/usr/bin:/sbin:/bin"
"#;

    #[test]
    fn exact_koreader_manifest_registers_by_stable_id() {
        let application = ValidatedApplicationManifest::from_toml(KOREADER).unwrap();
        assert_eq!(
            application.manifest().display.mode,
            DisplayMode::Framebuffer
        );
        assert_eq!(
            application.manifest().display.handoff,
            DisplayHandoff::Supervisor
        );
        assert_eq!(
            application.manifest().lifecycle.restart,
            RestartPolicy::Never
        );
        assert_eq!(
            application
                .manifest()
                .environment
                .get("PATH")
                .map(String::as_str),
            Some("/usr/sbin:/usr/bin:/sbin:/bin")
        );

        let mut catalog = ApplicationCatalog::default();
        catalog.register(application.clone()).unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.iter().next(), Some(&application));
        assert_eq!(
            catalog.register(application),
            Err(CatalogError::DuplicateId("org.koreader.reader".to_owned()))
        );
    }

    #[test]
    fn two_application_catalog_is_sorted_by_stable_id() {
        let mut catalog = ApplicationCatalog::default();
        catalog
            .register(ValidatedApplicationManifest::from_toml(KOREADER).unwrap())
            .unwrap();
        catalog
            .register(ValidatedApplicationManifest::from_toml(HOME_ASSISTANT).unwrap())
            .unwrap();

        let ids: Vec<_> = catalog
            .iter()
            .map(|application| application.manifest().id.as_str())
            .collect();
        assert_eq!(ids, ["io.home-assistant.dashboard", "org.koreader.reader"]);
    }

    #[test]
    fn unknown_fields_and_versions_fail_closed() {
        let unknown = KOREADER.replace("name = \"KOReader\"", "name = \"KOReader\"\nmagic = true");
        assert!(matches!(
            ValidatedApplicationManifest::from_toml(&unknown),
            Err(ManifestError::Toml(_))
        ));

        let missing_description = KOREADER.replace(
            "description = \"Open your library and continue reading\"\n",
            "",
        );
        assert!(matches!(
            ValidatedApplicationManifest::from_toml(&missing_description),
            Err(ManifestError::Toml(_))
        ));

        let missing_icon = KOREADER.replace(
            "icon = \"/mnt/us/ferrink/apps/org.koreader.reader/icon.png\"\n",
            "",
        );
        assert!(matches!(
            ValidatedApplicationManifest::from_toml(&missing_icon),
            Err(ManifestError::Toml(_))
        ));

        let version = KOREADER.replace("manifest_version = 1", "manifest_version = 2");
        let Err(ManifestError::Validation(violations)) =
            ValidatedApplicationManifest::from_toml(&version)
        else {
            panic!("unsupported version did not fail semantic validation");
        };
        assert!(
            violations
                .iter()
                .any(|issue| issue.field == "manifest_version")
        );
    }

    #[test]
    fn command_ids_deadlines_and_services_are_bounded() {
        let invalid = KOREADER
            .replace(
                "/mnt/us/ferrink/apps/org.koreader.reader/launch-kindle.sh",
                "../koreader.sh",
            )
            .replace("org.koreader.reader", "KOReader")
            .replace(
                "stock_services = [\"com.lab126.powerd\"]",
                "stock_services = [\"same\", \"same\"]",
            )
            .replace(
                "startup_timeout_seconds = 20",
                "startup_timeout_seconds = 0",
            )
            .replace("exit_timeout_seconds = 10", "exit_timeout_seconds = 61");
        let Err(ManifestError::Validation(violations)) =
            ValidatedApplicationManifest::from_toml(&invalid)
        else {
            panic!("invalid manifest did not fail semantic validation");
        };
        for field in [
            "id",
            "command[0]",
            "requirements.stock_services",
            "lifecycle.startup_timeout_seconds",
            "lifecycle.exit_timeout_seconds",
        ] {
            assert!(violations.iter().any(|issue| issue.field == field));
        }
    }

    #[test]
    fn icon_must_be_a_clean_absolute_png_path() {
        for invalid in [
            "../icon.png",
            "/mnt/us/ferrink/apps/org.koreader.reader/../icon.png",
            "/mnt/us/ferrink/apps/org.koreader.reader/icon.svg",
        ] {
            let manifest =
                KOREADER.replace("/mnt/us/ferrink/apps/org.koreader.reader/icon.png", invalid);
            let Err(ManifestError::Validation(violations)) =
                ValidatedApplicationManifest::from_toml(&manifest)
            else {
                panic!("invalid icon path {invalid:?} passed validation");
            };
            assert!(violations.iter().any(|issue| issue.field == "icon"));
        }
    }

    #[test]
    fn environment_rejects_injection_and_relative_paths() {
        let injected = KOREADER.replace(
            "PATH = \"/usr/sbin:/usr/bin:/sbin:/bin\"",
            "PATH = \"/usr/bin:.\"\nLD_PRELOAD = \"/tmp/inject.so\"",
        );
        let Err(ManifestError::Validation(violations)) =
            ValidatedApplicationManifest::from_toml(&injected)
        else {
            panic!("unsafe environment did not fail semantic validation");
        };
        assert!(
            violations
                .iter()
                .any(|issue| issue.field == "environment.PATH")
        );
        assert!(violations.iter().any(|issue| issue.field == "environment"));
    }

    #[test]
    fn oversized_input_fails_before_parsing() {
        let input = "x".repeat(usize::try_from(MAX_MANIFEST_BYTES).unwrap() + 1);
        assert!(matches!(
            ValidatedApplicationManifest::from_toml(&input),
            Err(ManifestError::InputTooLarge)
        ));
    }
}
