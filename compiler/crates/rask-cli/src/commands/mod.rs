// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! CLI command implementations.

pub mod phase;
pub mod analysis;
pub mod codegen;
pub mod compile;
pub mod run;
pub mod sim;
pub mod agents;
pub mod claims;
pub mod claims_git;
pub mod claims_tools;
pub mod mcp;
pub mod build;
pub mod link;
pub mod tools;
pub mod specs;
pub mod coverage;
pub mod deps;
pub mod manifest;
pub mod watch;
pub mod cache;
pub mod init;
pub mod fetch;
pub mod vendor;
pub mod publish;
pub mod audit;
pub mod keys;

/// A file in the temp directory that no other process holds. It's created
/// exclusively, so two `rask` commands never share one, even with the same
/// process id in different containers over one `/tmp`. The caller removes it.
pub(crate) fn own_temp_file(stem: &str, ext: &str) -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir();
    for n in 0..1000u32 {
        let path = dir.join(format!("rask-{}-{}-{}.{}", stem, std::process::id(), n, ext));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free temporary file name"))
}

#[cfg(test)]
mod tests {
    /// A name another process already holds, as one with the same pid in
    /// another container over the same `/tmp` would, is skipped.
    #[test]
    fn own_temp_file_skips_a_name_already_taken() {
        let stem = format!("owntest-{}", std::process::id());
        let taken = std::env::temp_dir().join(format!("rask-{}-{}-0.tmp", stem, std::process::id()));
        std::fs::create_dir_all(&taken).unwrap();
        let got = super::own_temp_file(&stem, "tmp").unwrap();
        assert_ne!(got, taken);
        assert!(got.is_file());
        let _ = std::fs::remove_file(&got);
        let _ = std::fs::remove_dir(&taken);
    }
}
