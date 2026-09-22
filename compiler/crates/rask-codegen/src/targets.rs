// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Rask's target names and the triples they mean (XT2, XT9).
//!
//! `--target aarch64-macos` used to go straight to `Triple::from_str`, which
//! parses leniently: it took the architecture and defaulted the rest, so the
//! object came out ELF on a name that names macOS. Nothing said so. The link
//! step failed for an unrelated reason (no cross-linker), which is the only
//! thing that kept a Mach-O-shaped mistake from reaching a linker that would
//! have taken it (#1185).
//!
//! So the short names are Rask's spelling and this is the one place that turns
//! one into a triple. A name that isn't here is rejected rather than guessed at.

use std::str::FromStr;
use target_lexicon::Triple;

/// Support tier from the spec's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Tested, guaranteed.
    One,
    /// Builds, best-effort.
    Two,
    /// Community.
    Three,
}

/// One target Rask names.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// What `--target` takes and `rask targets` prints.
    pub name: &'static str,
    /// The triple codegen is built for.
    pub triple: &'static str,
    pub tier: Tier,
}

/// Every target `rask targets` prints, in the order it prints them.
pub const TARGETS: &[Target] = &[
    Target { name: "x86_64-linux", triple: "x86_64-unknown-linux-gnu", tier: Tier::One },
    Target { name: "aarch64-linux", triple: "aarch64-unknown-linux-gnu", tier: Tier::One },
    Target { name: "x86_64-macos", triple: "x86_64-apple-darwin", tier: Tier::One },
    Target { name: "aarch64-macos", triple: "aarch64-apple-darwin", tier: Tier::One },
    Target { name: "x86_64-windows-msvc", triple: "x86_64-pc-windows-msvc", tier: Tier::Two },
    Target { name: "aarch64-windows-msvc", triple: "aarch64-pc-windows-msvc", tier: Tier::Two },
    Target { name: "wasm32-none", triple: "wasm32-unknown-unknown", tier: Tier::Two },
    Target { name: "x86_64-linux-musl", triple: "x86_64-unknown-linux-musl", tier: Tier::Two },
    Target { name: "aarch64-linux-musl", triple: "aarch64-unknown-linux-musl", tier: Tier::Two },
    Target { name: "riscv64-linux", triple: "riscv64gc-unknown-linux-gnu", tier: Tier::Three },
    Target { name: "x86_64-freebsd", triple: "x86_64-unknown-freebsd", tier: Tier::Three },
    Target { name: "arm-none", triple: "armv7-unknown-none-eabihf", tier: Tier::Three },
];

/// The triple to build codegen for, from a `--target` name.
///
/// A full triple is accepted as written — `aarch64-apple-darwin` always worked
/// and people have it in scripts. Anything else is an error: letting it through
/// is how a name got a binary format nobody asked for.
pub fn codegen_triple(name: &str) -> Result<&str, String> {
    if let Some(t) = TARGETS.iter().find(|t| t.name == name) {
        return Ok(t.triple);
    }
    if TARGETS.iter().any(|t| t.triple == name) {
        return Ok(name);
    }
    // A vendor field is what separates a real triple from one of our names:
    // `aarch64-apple-darwin` has one, `aarch64-macos` doesn't, and it's the
    // missing field that made the parse fall back to defaults.
    if name.split('-').count() >= 3 && Triple::from_str(name).is_ok() {
        return Ok(name);
    }
    Err(format!(
        "unknown target '{}' — run `rask targets` to see available targets",
        name,
    ))
}

/// The architecture and OS a target name means, in Rask's own spellings.
///
/// Splitting a name on '-' and calling field 1 the OS is right for
/// `aarch64-macos` and wrong for `aarch64-apple-darwin`, which came out as OS
/// "apple" — so the link step refused a triple codegen had just built for
/// (#1185). Both spellings go through the table.
pub fn arch_and_os(name: &str) -> Result<(String, String), String> {
    use target_lexicon::OperatingSystem;
    let triple: Triple = codegen_triple(name)?
        .parse()
        .map_err(|e| format!("invalid target '{}': {}", name, e))?;
    let arch = triple.architecture.to_string();
    // The cross-compiler prefixes and the runtime's source lists are keyed on
    // Rask's spelling, not on the sub-architecture a triple carries.
    let arch = if arch.starts_with("armv") {
        "arm".to_string()
    } else if arch.starts_with("riscv64") {
        "riscv64".to_string()
    } else {
        arch
    };
    let os = match triple.operating_system {
        OperatingSystem::Linux => "linux",
        OperatingSystem::Darwin | OperatingSystem::MacOSX { .. } => "macos",
        OperatingSystem::Windows => "windows",
        OperatingSystem::Freebsd => "freebsd",
        OperatingSystem::None_ | OperatingSystem::Unknown => "none",
        other => {
            return Err(format!(
                "cross-compilation to {} — runtime not available for OS '{}'",
                name, other,
            ))
        }
    };
    Ok((arch, os.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use target_lexicon::{Architecture, BinaryFormat, OperatingSystem};

    /// The bug was silence: `aarch64-macos` produced an object with the right
    /// architecture and the wrong format. So the assertion is on the format,
    /// which is the thing a linker rejects.
    #[test]
    fn every_target_name_means_the_format_its_name_says() {
        let want = |name: &str| -> BinaryFormat {
            if name.contains("macos") {
                BinaryFormat::Macho
            } else if name.contains("windows") {
                BinaryFormat::Coff
            } else if name.starts_with("wasm32") {
                BinaryFormat::Wasm
            } else {
                // Linux, FreeBSD and bare metal all use ELF.
                BinaryFormat::Elf
            }
        };
        for t in TARGETS {
            let triple = Triple::from_str(codegen_triple(t.name).unwrap()).unwrap();
            assert_eq!(
                triple.binary_format,
                want(t.name),
                "{} → {}",
                t.name,
                t.triple,
            );
        }
    }

    #[test]
    fn the_architecture_survives_the_mapping() {
        for t in TARGETS {
            let triple = Triple::from_str(codegen_triple(t.name).unwrap()).unwrap();
            let arch = t.name.split('-').next().unwrap();
            let got = triple.architecture;
            let ok = match arch {
                "x86_64" => got == Architecture::X86_64,
                "aarch64" => matches!(got, Architecture::Aarch64(_)),
                "wasm32" => got == Architecture::Wasm32,
                "riscv64" => matches!(got, Architecture::Riscv64(_)),
                "arm" => matches!(got, Architecture::Arm(_)),
                other => panic!("no expectation for architecture '{}'", other),
            };
            assert!(ok, "{} came out as {:?}", t.name, got);
        }
    }

    /// The four Tier 1 targets are the ones a release ships, so their OS has to
    /// be the one named and Cranelift has to have a backend for them.
    #[test]
    fn tier_one_targets_have_a_backend() {
        for t in TARGETS.iter().filter(|t| t.tier == Tier::One) {
            let triple = Triple::from_str(t.triple).unwrap();
            let named_macos = t.name.contains("macos");
            let is_macos = matches!(
                triple.operating_system,
                OperatingSystem::Darwin | OperatingSystem::MacOSX { .. },
            );
            assert_eq!(named_macos, is_macos, "{}", t.name);
            assert!(
                cranelift_codegen::isa::lookup(triple).is_ok(),
                "no Cranelift backend for {}",
                t.triple,
            );
        }
    }

    /// The full triple and the short name have to agree, which is the half of
    /// #1185 that made the link step refuse a Mach-O object it had just built.
    #[test]
    fn both_spellings_of_a_target_answer_the_same_arch_and_os() {
        for t in TARGETS {
            assert_eq!(
                arch_and_os(t.name).ok(),
                arch_and_os(t.triple).ok(),
                "{} vs {}",
                t.name,
                t.triple,
            );
        }
        assert_eq!(
            arch_and_os("aarch64-apple-darwin").unwrap(),
            ("aarch64".to_string(), "macos".to_string()),
        );
    }

    #[test]
    fn a_name_nobody_declared_is_an_error_not_a_guess() {
        assert!(codegen_triple("aarch64-mac").is_err());
        assert!(codegen_triple("sparc-solaris").is_err());
        // A real triple still passes straight through.
        assert_eq!(
            codegen_triple("aarch64-apple-darwin").unwrap(),
            "aarch64-apple-darwin",
        );
    }
}
