// SPDX-License-Identifier: Apache-2.0

//! Minima `.jar` acquisition and verification.
//!
//! The crate **does not vendor** the `.jar`. It ships an in-crate
//! [`PINNED`] manifest mapping a specific upstream release tag to the
//! expected SHA256. At `init` time, the jar is either downloaded from
//! `github.com/minima-global/Minima` or supplied via `--jar <path>`,
//! and in both cases is verified against the pinned SHA256 before being
//! installed under `/usr/lib/furcate-pi-minima/`.
//!
//! Bumping the pinned Minima version = a new release of this crate
//! (not a runtime upgrade path). That keeps the dependency surface
//! explicit and reviewable.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::Error;

/// The Minima release this crate version is pinned to.
///
/// 0.1.0 of `furcate-pi-minima` ships against Minima v1.0.45
/// (2025-03-28, the current stable as of crate authoring). Operators
/// who need a newer Minima version must wait for a crate version bump,
/// or use `Profile::Custom` with a manually placed jar.
pub const PINNED: PinnedJar = PinnedJar {
    version: "v1.0.45",
    // SHA-256 of the v1.0.45 release asset, published inline in the
    // GitHub release body (Minima publishes hashes as `0x`-prefixed
    // uppercase; we store lowercase, unprefixed — standard sha256sum
    // form). Source: github.com/minima-global/Minima/releases/tag/v1.0.45
    //
    // Upstream labels this hash `minima-1.0.45.15.jar`, but the
    // downloaded asset is named `minima.jar` — same bytes, the version
    // lives in the JAR manifest, not the filename. See
    // `docs/minima-reference.md` §1.
    sha256_hex: "241f9429d0ea2599905fe0950e0dde4c59a2cabe3e480cd89b6e24a628cee56a",
    // Tagged release URL (NOT `master`) for reproducibility. The
    // upstream systemd guide uses the `master` raw URL; we override
    // because we pin SHA256 against a specific tag.
    url: "https://github.com/minima-global/Minima/releases/download/v1.0.45/minima.jar",
};

/// A pinned upstream Minima jar.
#[derive(Clone, Copy, Debug)]
pub struct PinnedJar {
    /// Upstream release tag, e.g. `"v1.0.45"`.
    pub version: &'static str,
    /// Expected SHA256 of the jar, lowercase hex.
    pub sha256_hex: &'static str,
    /// Direct download URL on `github.com/minima-global/Minima`.
    pub url: &'static str,
}

/// Compute the SHA256 of a file on disk, returning lowercase hex.
///
/// Streamed read — works on any Pi without loading the full ~70 MB
/// jar into memory.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file can't be opened or read.
pub async fn sha256_file(path: &Path) -> Result<String, Error> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verify a jar on disk against the pinned manifest.
///
/// # Errors
///
/// Returns [`Error::JarHashMismatch`] with both the expected and actual
/// hex so the operator can see exactly what they got. Returns
/// [`Error::Io`] if the file can't be read.
pub async fn verify(path: &Path, pinned: &PinnedJar) -> Result<(), Error> {
    let actual = sha256_file(path).await?;
    if actual != pinned.sha256_hex {
        return Err(Error::JarHashMismatch {
            path: path.to_path_buf(),
            expected: pinned.sha256_hex.into(),
            actual,
        });
    }
    Ok(())
}

/// Download the pinned jar to `dest`, with a progress bar on stderr.
///
/// Verifies SHA256 against [`PinnedJar::sha256_hex`] before returning.
/// Refuses to leave a partial download in place — on failure, removes
/// the temp file.
///
/// Strategy:
/// 1. GET `pinned.url`, stream to a sibling tempfile.
/// 2. Drive an `indicatif::ProgressBar` from `Content-Length`.
/// 3. `sha256_file` the tempfile; on mismatch, unlink + bubble
///    [`Error::JarHashMismatch`].
/// 4. `fs::rename` tempfile → dest (atomic on the same filesystem).
///
/// # Errors
///
/// Returns [`Error::Http`] on a non-2xx response or transport failure,
/// [`Error::Io`] on local disk errors, and [`Error::JarHashMismatch`]
/// if the downloaded bytes don't match the pinned SHA256.
///
/// # Panics
///
/// Panics if the in-crate `indicatif::ProgressStyle` template is
/// malformed — that's a static template, so a panic here is a
/// build-time bug in this crate, not an operator-facing condition.
pub async fn download_and_verify(
    pinned: &PinnedJar,
    dest: &Path,
    client: &reqwest::Client,
) -> Result<(), Error> {
    use futures::StreamExt;
    use indicatif::{ProgressBar, ProgressStyle};
    use tokio::io::AsyncWriteExt;

    let parent = dest.parent().ok_or_else(|| {
        Error::Preflight(format!("destination has no parent dir: {}", dest.display()))
    })?;
    tokio::fs::create_dir_all(parent).await?;

    let tmp = parent.join(format!(
        ".{}.partial",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("minima.jar"),
    ));

    let resp = client.get(pinned.url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);

    let pb = if total > 0 {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} downloading minima.jar [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("=> "),
        );
        Some(bar)
    } else {
        None
    };

    {
        let mut out = tokio::fs::File::create(&tmp).await?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            out.write_all(&chunk).await?;
            if let Some(ref bar) = pb {
                bar.inc(chunk.len() as u64);
            }
        }
        out.sync_all().await?;
    }
    if let Some(bar) = pb {
        bar.finish_and_clear();
    }

    // Verify before promoting tmp → dest. On mismatch, the partial
    // file is dropped and the error tells the operator exactly which
    // bytes don't match.
    if let Err(e) = verify(&tmp, pinned).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }

    // Atomic on the same filesystem; the rename happens after sha256
    // verify, so no caller ever sees an unverified jar at `dest`.
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sha256_of_known_input() {
        // SHA256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();
        let h = sha256_file(&path).await.unwrap();
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[tokio::test]
    async fn verify_rejects_wrong_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrong.jar");
        tokio::fs::write(&path, b"not the real jar").await.unwrap();
        let pinned = PinnedJar {
            version: "test",
            sha256_hex: "0000000000000000000000000000000000000000000000000000000000000000",
            url: "https://example.invalid/none",
        };
        let err = verify(&path, &pinned).await.unwrap_err();
        assert!(matches!(err, Error::JarHashMismatch { .. }));
    }
}
