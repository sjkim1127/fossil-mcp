use crate::error::RepoError;
use git2::{DiffOptions, Repository, Tree};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Represents the changed lines and contents of a file between two revisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub file_path: String,
    pub before_content: String,
    pub after_content: String,
    /// Line numbers in the 'before' content that were changed or removed.
    pub changed_lines_before: Vec<usize>,
    /// Line numbers in the 'after' content that were changed or added.
    pub changed_lines_after: Vec<usize>,
}

/// Retrieves the raw file contents from a specific tree for a given path.
fn get_file_content_from_tree(
    repo: &Repository,
    tree: &Tree,
    path: &str,
) -> Result<String, RepoError> {
    let entry = tree.get_path(Path::new(path)).map_err(|_| {
        RepoError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Path not found in tree",
        ))
    })?;
    let object = entry.to_object(repo).map_err(RepoError::Git)?;
    if let Some(blob) = object.as_blob() {
        if blob.is_binary() {
            return Ok(String::new());
        }
        let content = std::str::from_utf8(blob.content()).unwrap_or("");
        Ok(content.to_string())
    } else {
        Ok(String::new())
    }
}

/// Computes the file changes between two revisions in a repository.
pub fn get_file_changes(
    repo_dir: &Path,
    start_rev: &str,
    end_rev: &str,
    file_pattern: &str, // Currently ignored or can be used to filter paths
) -> Result<Vec<FileChange>, RepoError> {
    let repo = Repository::open(repo_dir)?;

    let rev1 = repo.revparse_single(start_rev)?;
    let rev2 = repo.revparse_single(end_rev)?;

    let tree1 = rev1.peel_to_tree()?;
    let tree2 = rev2.peel_to_tree()?;

    let mut diff_opts = DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&tree1), Some(&tree2), Some(&mut diff_opts))?;

    let mut changes = std::collections::HashMap::new();

    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
            // Very naive filter matching:
            if !file_pattern.is_empty() && !path.ends_with(file_pattern.trim_start_matches('*')) {
                return true; // Skip
            }

            let entry = changes.entry(path.to_string()).or_insert_with(|| {
                // Fetch the full before and after contents
                let before_content =
                    get_file_content_from_tree(&repo, &tree1, path).unwrap_or_default();
                let after_content =
                    get_file_content_from_tree(&repo, &tree2, path).unwrap_or_default();
                FileChange {
                    file_path: path.to_string(),
                    before_content,
                    after_content,
                    changed_lines_before: Vec::new(),
                    changed_lines_after: Vec::new(),
                }
            });

            match line.origin() {
                '-' | '<' => {
                    if let Some(ln) = line.old_lineno() {
                        entry.changed_lines_before.push(ln as usize);
                    }
                }
                '+' | '>' => {
                    if let Some(ln) = line.new_lineno() {
                        entry.changed_lines_after.push(ln as usize);
                    }
                }
                _ => {}
            }
        }
        true
    })
    .map_err(RepoError::Git)?;

    Ok(changes.into_values().collect())
}
