use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha512};

use crate::{
    error::{GitxError, Result},
    http, report,
};

const SHA512_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportOutcome {
    AlreadyCached(PathBuf),
    Imported(PathBuf),
}

pub fn ensure_tarball(
    cache_root: impl AsRef<Path>,
    tarball_url: &str,
    integrity: &str,
) -> Result<PathBuf> {
    let path = tarball_path(cache_root, integrity)?;

    if path.exists() {
        match verify_file(&path, integrity) {
            Ok(()) => return Ok(path),
            Err(_) => fs::remove_file(&path).map_err(|source| GitxError::Io {
                context: format!("removing corrupt cached tarball {}", path.display()),
                source,
            })?,
        }
    }

    let parent = path
        .parent()
        .ok_or_else(|| GitxError::Message(format!("invalid cache path {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|source| GitxError::Io {
        context: format!("creating artifact cache directory {}", parent.display()),
        source,
    })?;

    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| GitxError::Io {
            context: format!("creating temporary tarball in {}", parent.display()),
            source,
        })?;
    let temporary_path = temporary.path().to_path_buf();
    download_to(tarball_url, temporary.as_file_mut(), &temporary_path)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| GitxError::Io {
            context: format!("syncing temporary tarball {}", temporary_path.display()),
            source,
        })?;
    verify_file(&temporary_path, integrity)?;
    persist_verified(temporary, &path, integrity)?;

    Ok(path)
}

pub fn tarball_path(cache_root: impl AsRef<Path>, integrity: &str) -> Result<PathBuf> {
    Ok(cache_root
        .as_ref()
        .join("npm")
        .join("tarballs")
        .join(stable_filename(integrity)?))
}

pub fn import_verified_tarball(
    cache_root: impl AsRef<Path>,
    source: impl AsRef<Path>,
    integrity: &str,
) -> Result<ImportOutcome> {
    let destination = tarball_path(cache_root, integrity)?;
    let source = source.as_ref();

    if destination.exists() {
        verify_file(&destination, integrity)?;
        return Ok(ImportOutcome::AlreadyCached(destination));
    }

    verify_file(source, integrity)?;

    let parent = destination.parent().ok_or_else(|| {
        GitxError::Message(format!("invalid cache path {}", destination.display()))
    })?;
    fs::create_dir_all(parent).map_err(|source| GitxError::Io {
        context: format!("creating artifact cache directory {}", parent.display()),
        source,
    })?;

    let mut input = fs::File::open(source).map_err(|io_error| GitxError::Io {
        context: format!("opening verified tarball {} for import", source.display()),
        source: io_error,
    })?;
    let source_size = input
        .metadata()
        .map_err(|io_error| GitxError::Io {
            context: format!("reading tarball size {}", source.display()),
            source: io_error,
        })?
        .len();
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| GitxError::Io {
            context: format!("creating temporary tarball in {}", parent.display()),
            source,
        })?;
    report::step("Importing a verified package tarball from the local cache");
    copy_with_progress(
        &mut input,
        &mut temporary,
        Some(source_size),
        "Importing cached package tarball",
    )
    .map_err(|io_error| GitxError::Io {
        context: format!(
            "copying verified tarball {} into {}",
            source.display(),
            parent.display()
        ),
        source: io_error,
    })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| GitxError::Io {
            context: format!("syncing imported tarball for {}", destination.display()),
            source,
        })?;
    verify_file(temporary.path(), integrity)?;

    if persist_verified(temporary, &destination, integrity)? {
        Ok(ImportOutcome::Imported(destination))
    } else {
        Ok(ImportOutcome::AlreadyCached(destination))
    }
}

pub fn integrity_sha512_hex(integrity: &str) -> Result<String> {
    let digest = sha512_integrity_digest(integrity)?;
    Ok(hex_encode(&digest))
}

pub fn verify_file(path: impl AsRef<Path>, integrity: &str) -> Result<()> {
    let expected = sha512_integrity_digest(integrity)?;
    let mut file = fs::File::open(path.as_ref()).map_err(|source| GitxError::Io {
        context: format!("opening tarball {}", path.as_ref().display()),
        source,
    })?;
    let actual = sha512_reader(&mut file).map_err(|source| GitxError::Io {
        context: format!("hashing tarball {}", path.as_ref().display()),
        source,
    })?;

    if actual != expected {
        return Err(GitxError::Message(format!(
            "tarball integrity mismatch for {}: expected sha512-{}, got sha512-{}",
            path.as_ref().display(),
            hex_encode(&expected),
            hex_encode(&actual)
        )));
    }

    Ok(())
}

fn stable_filename(integrity: &str) -> Result<String> {
    let digest = sha512_integrity_digest(integrity)?;
    Ok(format!("sha512-{}.tgz", hex_encode(&digest)))
}

fn download_to(tarball_url: &str, writer: &mut impl Write, path: &Path) -> Result<()> {
    let mut response = http::client()?
        .get(tarball_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|source| GitxError::Http {
            context: format!("downloading tarball {tarball_url}"),
            source,
        })?;

    report::step("Downloading a package tarball into the GitX cache");
    let total = response.content_length();
    copy_with_progress(&mut response, writer, total, "Downloading package tarball").map_err(
        |source| GitxError::Io {
            context: format!("writing temporary tarball {}", path.display()),
            source,
        },
    )?;

    Ok(())
}

fn copy_with_progress(
    reader: &mut impl Read,
    writer: &mut impl Write,
    total: Option<u64>,
    label: &str,
) -> io::Result<()> {
    let progress = report::transfer(label, total);
    let mut buffer = [0; 32 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        progress.advance(read as u64);
    }

    progress.finish();
    Ok(())
}

fn persist_verified(
    temporary: tempfile::NamedTempFile,
    destination: &Path,
    integrity: &str,
) -> Result<bool> {
    match temporary.persist_noclobber(destination) {
        Ok(_) => Ok(true),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            // A concurrent writer won the race. Its file is reusable only if
            // it independently satisfies the same content identity.
            verify_file(destination, integrity)?;
            Ok(false)
        }
        Err(error) => Err(GitxError::Io {
            context: format!(
                "persisting verified tarball without overwriting {}",
                destination.display()
            ),
            source: error.error,
        }),
    }
}

fn sha512_integrity_digest(integrity: &str) -> Result<[u8; SHA512_LEN]> {
    let token = integrity
        .split_whitespace()
        .find(|token| token.starts_with("sha512-"))
        .ok_or_else(|| {
            GitxError::Message(format!(
                "unsupported npm integrity `{integrity}`: expected sha512 SRI"
            ))
        })?;
    let encoded = token
        .strip_prefix("sha512-")
        .unwrap_or(token)
        .split_once('?')
        .map(|(value, _)| value)
        .unwrap_or_else(|| token.strip_prefix("sha512-").unwrap_or(token));
    let decoded = STANDARD.decode(encoded).map_err(|source| {
        GitxError::Message(format!(
            "invalid base64 in npm integrity `{integrity}`: {source}"
        ))
    })?;

    if decoded.len() != SHA512_LEN {
        return Err(GitxError::Message(format!(
            "invalid sha512 integrity length for `{integrity}`: expected {SHA512_LEN} bytes, got {}",
            decoded.len()
        )));
    }

    let mut digest = [0; SHA512_LEN];
    digest.copy_from_slice(&decoded);
    Ok(digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }

    encoded
}

fn sha512_reader(reader: &mut impl Read) -> io::Result<[u8; SHA512_LEN]> {
    let mut hasher = Sha512::new();
    let mut buffer = [0; 32 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

    use super::{
        ImportOutcome, ensure_tarball, import_verified_tarball, sha512_reader, tarball_path,
        verify_file,
    };

    const HELLO_INTEGRITY: &str = "sha512-m3HSJL1i83hdltRq0+o9czGb+8KJDKra4t/3JRlnPKcjI8PZm6XBHXx6zG4UuMXaDEZjR1wuXDre9G9zvN7AQw==";
    const HELLO_SHA512_HEX: &str = "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043";

    #[test]
    fn derives_stable_tarball_path_from_integrity_digest() {
        let tempdir = tempfile::tempdir().unwrap();

        let path = tarball_path(tempdir.path(), HELLO_INTEGRITY).unwrap();

        assert_eq!(
            path,
            tempdir
                .path()
                .join("npm")
                .join("tarballs")
                .join(format!("sha512-{HELLO_SHA512_HEX}.tgz"))
        );
    }

    #[test]
    fn verifies_sha512_integrity_for_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("artifact.tgz");
        fs::write(&path, b"hello").unwrap();

        verify_file(&path, HELLO_INTEGRITY).unwrap();
    }

    #[test]
    fn rejects_integrity_mismatch() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("artifact.tgz");
        fs::write(&path, b"not hello").unwrap();

        let error = verify_file(&path, HELLO_INTEGRITY).unwrap_err();

        assert!(error.to_string().contains("tarball integrity mismatch"));
    }

    #[test]
    fn hashes_sha512_known_vector() {
        let mut input: &[u8] = b"hello";

        let digest = sha512_reader(&mut input).unwrap();

        assert_eq!(hex_for_test(&digest), HELLO_SHA512_HEX);
    }

    #[test]
    fn downloads_tarball_once_and_reuses_verified_cache() {
        let tempdir = tempfile::tempdir().unwrap();
        let (url, requests, handle) = serve_once(b"hello".to_vec());

        let first = ensure_tarball(tempdir.path(), &url, HELLO_INTEGRITY).unwrap();
        let second = ensure_tarball(tempdir.path(), &url, HELLO_INTEGRITY).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"hello");
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        handle.join().unwrap();
    }

    #[test]
    fn imports_verified_tarball_by_copy() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("source.tgz");
        fs::write(&source, b"hello").unwrap();

        let imported = import_verified_tarball(tempdir.path(), &source, HELLO_INTEGRITY).unwrap();
        let cached = tarball_path(tempdir.path(), HELLO_INTEGRITY).unwrap();

        assert_eq!(imported, ImportOutcome::Imported(cached.clone()));
        assert_eq!(fs::read(&source).unwrap(), b"hello");
        assert_eq!(fs::read(&cached).unwrap(), b"hello");

        let second = import_verified_tarball(tempdir.path(), &source, HELLO_INTEGRITY).unwrap();

        assert_eq!(second, ImportOutcome::AlreadyCached(cached));
    }

    #[test]
    fn concurrent_tarball_imports_share_one_verified_destination() {
        let tempdir = tempfile::tempdir().unwrap();
        let source = tempdir.path().join("source.tgz");
        fs::write(&source, b"hello").unwrap();
        let cache = tempdir.path().join("cache");
        let barrier = Arc::new(Barrier::new(2));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let cache = cache.clone();
                let source = source.clone();
                thread::spawn(move || {
                    barrier.wait();
                    import_verified_tarball(cache, source, HELLO_INTEGRITY)
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        verify_file(
            tarball_path(&cache, HELLO_INTEGRITY).unwrap(),
            HELLO_INTEGRITY,
        )
        .unwrap();
    }

    fn serve_once(body: Vec<u8>) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let thread_requests = Arc::clone(&requests);

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            thread_requests.fetch_add(1, Ordering::SeqCst);

            let mut request_buffer = [0; 1024];
            let _ = stream.read(&mut request_buffer).unwrap();

            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });

        (format!("http://{address}/package.tgz"), requests, handle)
    }

    fn hex_for_test(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
