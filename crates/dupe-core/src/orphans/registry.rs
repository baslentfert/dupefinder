//! Reading "what is installed on this machine" out of the registry.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

/// Where Windows records installed software.
///
/// All three views matter: 32-bit installers land under `WOW6432Node`, and
/// per-user installs never appear in `HKLM` at all.
fn uninstall_keys() -> Vec<(RegKey, &'static str)> {
    vec![
        (
            RegKey::predef(HKEY_LOCAL_MACHINE),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            RegKey::predef(HKEY_LOCAL_MACHINE),
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            RegKey::predef(HKEY_CURRENT_USER),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ]
}

/// Normalised names of everything that appears to be installed.
///
/// Used only for fuzzy matching, so it deliberately over-collects: publisher
/// names, product names and install directories all go in the same bag. A
/// false match means a leftover folder is not reported, which is the failure
/// direction we want.
#[derive(Debug, Default)]
pub struct InstalledProducts {
    tokens: HashSet<String>,
}

impl InstalledProducts {
    /// How many distinct names were found. A suspiciously small number means
    /// the registry could not be read properly and callers should not draw
    /// conclusions from "no match".
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Whether `name` plausibly belongs to something still installed.
    ///
    /// Matching is substring-based in both directions, because the folder an
    /// application writes to rarely equals its display name: "JetBrains
    /// Toolbox" writes to `JetBrains`, and `Adobe Acrobat DC` writes to
    /// `Adobe`.
    pub fn matches(&self, name: &str) -> bool {
        let needle = normalise(name);
        // Below four characters, substring matching produces nonsense: "ea"
        // matches half the registry.
        if needle.len() < 4 {
            return true;
        }

        self.tokens
            .iter()
            .any(|token| token.contains(&needle) || needle.contains(token.as_str()))
    }

    fn insert(&mut self, value: &str) {
        let token = normalise(value);
        if token.len() >= 4 {
            self.tokens.insert(token);
        }
    }
}

/// Collect installed product names from the uninstall registry and from the
/// directories applications actually install into.
pub fn installed_products() -> InstalledProducts {
    let mut products = InstalledProducts::default();

    for (root, path) in uninstall_keys() {
        let Ok(key) = root.open_subkey_with_flags(path, KEY_READ) else {
            continue;
        };

        for name in key.enum_keys().flatten() {
            let Ok(entry) = key.open_subkey_with_flags(&name, KEY_READ) else {
                continue;
            };

            for value in ["DisplayName", "Publisher"] {
                if let Ok(text) = entry.get_value::<String, _>(value) {
                    products.insert(&text);
                }
            }

            if let Ok(location) = entry.get_value::<String, _>("InstallLocation") {
                for component in Path::new(&location).components() {
                    products.insert(&component.as_os_str().to_string_lossy());
                }
            }
        }
    }

    for directory in install_directories() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                products.insert(&entry.file_name().to_string_lossy());
            }
        }
    }

    products
}

fn install_directories() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .map(PathBuf::from)
        .chain(
            std::env::var("LOCALAPPDATA")
                .ok()
                .map(|p| PathBuf::from(p).join("Programs")),
        )
        .filter(|p| p.is_dir())
        .collect()
}

/// Lowercase, alphanumerics only. Turns `Adobe Acrobat (64-bit)` and
/// `Adobe-Acrobat` into the same token.
pub fn normalise(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Every `.msi` / `.msp` in the installer cache that a product or patch
/// registration still points at.
///
/// Returns the bare file names, lowercased.
pub fn referenced_installer_packages() -> std::io::Result<HashSet<String>> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let user_data = hklm.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData",
        KEY_READ,
    )?;

    let mut referenced = HashSet::new();

    // The tree is UserData\<user SID>\Products\<product code>\... , with
    // patches registered both under each product and per user.
    for sid in user_data.enum_keys().flatten() {
        let Ok(user) = user_data.open_subkey_with_flags(&sid, KEY_READ) else {
            continue;
        };

        if let Ok(products) = user.open_subkey_with_flags("Products", KEY_READ) {
            for code in products.enum_keys().flatten() {
                let Ok(product) = products.open_subkey_with_flags(&code, KEY_READ) else {
                    continue;
                };

                if let Ok(properties) =
                    product.open_subkey_with_flags("InstallProperties", KEY_READ)
                {
                    collect_local_package(&properties, &mut referenced);
                }

                if let Ok(patches) = product.open_subkey_with_flags("Patches", KEY_READ) {
                    for patch_code in patches.enum_keys().flatten() {
                        if let Ok(patch) = patches.open_subkey_with_flags(&patch_code, KEY_READ) {
                            collect_local_package(&patch, &mut referenced);
                        }
                    }
                }
            }
        }

        if let Ok(patches) = user.open_subkey_with_flags("Patches", KEY_READ) {
            for patch_code in patches.enum_keys().flatten() {
                if let Ok(patch) = patches.open_subkey_with_flags(&patch_code, KEY_READ) {
                    collect_local_package(&patch, &mut referenced);
                }
            }
        }
    }

    Ok(referenced)
}

fn collect_local_package(key: &RegKey, out: &mut HashSet<String>) {
    for value in ["LocalPackage", "ManagedLocalPackage"] {
        if let Ok(path) = key.get_value::<String, _>(value) {
            if let Some(name) = Path::new(&path).file_name() {
                out.insert(name.to_string_lossy().to_ascii_lowercase());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_collapses_punctuation_and_case() {
        assert_eq!(normalise("Adobe Acrobat (64-bit)"), "adobeacrobat64bit");
        assert_eq!(normalise("Adobe-Acrobat"), normalise("adobe acrobat"));
    }

    #[test]
    fn matching_works_in_both_directions() {
        let mut products = InstalledProducts::default();
        products.insert("JetBrains Toolbox");
        products.insert("Mozilla");

        // Folder name is a prefix of the product name.
        assert!(products.matches("JetBrains"));
        // Product name is a prefix of the folder name.
        assert!(products.matches("Mozilla Firefox"));
        assert!(!products.matches("SomeDefunctVendor"));
    }

    #[test]
    fn very_short_names_are_never_flagged() {
        let products = InstalledProducts::default();
        // Three characters cannot be matched reliably, so assume innocence.
        assert!(products.matches("EA"));
        assert!(products.matches("Adb"));
    }

    #[test]
    fn the_real_registry_lists_something() {
        // A machine with fewer than a handful of installed products does not
        // exist; if this is empty the reading logic is broken, and the
        // appdata detector must not draw conclusions from it.
        let products = installed_products();
        assert!(products.len() > 5, "found only {} products", products.len());
    }
}
