//! Local-path and `file://` URI source resolution.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::ToolError;

pub(super) fn read_file_uri(uri: &str) -> Result<Vec<u8>, ToolError> {
    let Some(rest) = uri.strip_prefix("file://") else {
        return Err(ToolError::invalid_source(uri, "file URI sources must start with file://"));
    };
    if rest.is_empty() {
        return Err(ToolError::invalid_source(uri, "file URI source path must not be empty"));
    }
    read_local_path(&PathBuf::from(rest), uri)
}

pub(super) fn read_local_path(path: &Path, source: &str) -> Result<Vec<u8>, ToolError> {
    if !path.is_absolute() {
        return Err(ToolError::invalid_source(source, "local source path must be absolute"));
    }
    let metadata = fs::metadata(path)
        .map_err(|err| ToolError::source_io("inspect local source", path, err))?;
    if !metadata.is_file() {
        return Err(ToolError::invalid_source(
            source,
            "local source path must resolve to a regular file",
        ));
    }
    fs::read(path).map_err(|err| ToolError::source_io("read local source", path, err))
}

#[cfg(test)]
mod tests {
    use super::super::resolve;
    use super::super::tests_common::*;
    use crate::error::ToolError;
    use crate::manifest::ToolSource;
    use crate::test_support::{scratch_dir, with_cache_env};

    #[test]
    fn file_uri_reuses_local_path_resolution() {
        let cache_dir = scratch_dir("resolver-file-cache");
        let source_dir = scratch_dir("resolver-file-source");
        let source = write_source(&source_dir, "module.wasm", b"file-uri");
        let scope = project_scope();
        let local = named_tool("local", ToolSource::LocalPath(source.clone()), None);
        let file_uri = named_tool(
            "file-uri",
            ToolSource::FileUri(format!("file://{}", source.display())),
            None,
        );

        with_cache_env(Some(&cache_dir), None, None, || {
            let local = resolve(&scope, &local, fixed_now()).expect("local resolves");
            let uri = resolve(&scope, &file_uri, fixed_now()).expect("file URI resolves");
            assert_eq!(std::fs::read(local.bytes_path).expect("local bytes"), b"file-uri");
            assert_eq!(std::fs::read(uri.bytes_path).expect("uri bytes"), b"file-uri");
        });
    }

    #[test]
    fn local_path_rejects_non_file_and_empty_file() {
        let cache_dir = scratch_dir("resolver-invalid-local-cache");
        let source_dir = scratch_dir("resolver-invalid-local-source");
        let empty = write_source(&source_dir, "empty.wasm", b"");
        let scope = project_scope();

        with_cache_env(Some(&cache_dir), None, None, || {
            let dir_err = resolve(
                &scope,
                &tool(ToolSource::LocalPath(source_dir.clone()), None),
                fixed_now(),
            )
            .expect_err("directory source must fail");
            assert!(matches!(dir_err, ToolError::InvalidSource { .. }), "{dir_err}");

            let empty_err = resolve(&scope, &tool(ToolSource::LocalPath(empty), None), fixed_now())
                .expect_err("empty file");
            assert!(matches!(empty_err, ToolError::EmptySource { .. }), "{empty_err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn local_path_chases_symlinks_to_regular_files() {
        let cache_dir = scratch_dir("resolver-symlink-cache");
        let source_dir = scratch_dir("resolver-symlink-source");
        let target = write_source(&source_dir, "target.wasm", b"symlink-target");
        let link = source_dir.join("link.wasm");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");
        let scope = project_scope();
        let symlink_tool = tool(ToolSource::LocalPath(link), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved = resolve(&scope, &symlink_tool, fixed_now()).expect("symlink resolves");
            assert_eq!(
                std::fs::read(resolved.bytes_path).expect("cached bytes"),
                b"symlink-target"
            );
        });
    }
}
