use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Binary,
    #[serde(rename = "non-utf8")]
    NonUtf8,
    #[serde(rename = "too-large")]
    TooLarge,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub section: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// Parse the `a/<old> b/<new>` remainder of a `diff --git ` line, used as
/// the provisional path source (overridden later by `--- `/`+++ ` or
/// `rename from`/`rename to` lines). Returns `None` for either side if the
/// expected `a/`/`b/` structure isn't found (e.g. unusual paths).
fn parse_diff_git_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(old) = rest.strip_prefix("a/") {
        if let Some(idx) = old.find(" b/") {
            let old_path = old[..idx].to_string();
            let new_path = old[idx + 3..].to_string();
            return (Some(old_path), Some(new_path));
        }
    }
    (None, None)
}

/// Strip a leading `a/` or `b/` prefix from a diff path, as used in
/// `--- a/foo` / `+++ b/foo` lines. `/dev/null` is not touched here;
/// callers check for it separately.
fn strip_ab_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

/// Parse a path from a `--- ` / `+++ ` header line's remainder.
/// Returns `None` for `/dev/null`, otherwise the path with `a/`/`b/` stripped.
fn parse_diff_path(rest: &str) -> Option<String> {
    // Git may append a tab and extra info (e.g. timestamps) after the path;
    // only the part before a tab is the path itself.
    let path_part = rest.split('\t').next().unwrap_or(rest).trim_end();
    if path_part == "/dev/null" {
        None
    } else {
        Some(strip_ab_prefix(path_part))
    }
}

/// Parse a unified hunk header of the form:
/// `@@ -{old_start}[,{old_count}] +{new_start}[,{new_count}] @@[ section]`
/// Returns (old_start, old_count, new_start, new_count, section).
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, section) = match rest.split_once(" @@") {
        Some((r, s)) => (r, s.trim_start().to_string()),
        None => return None,
    };
    let mut parts = ranges.split_whitespace();
    let old_range = parts.next()?;
    let new_range = parts.next()?;

    let (old_start, old_count) = parse_range(old_range.strip_prefix('-')?)?;
    let (new_start, new_count) = parse_range(new_range.strip_prefix('+')?)?;

    Some((old_start, old_count, new_start, new_count, section))
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

pub fn parse_unified_diff(input: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let (old_path, new_path) = parse_diff_git_paths(rest);
            files.push(FileDiff {
                old_path,
                new_path,
                status: FileStatus::Modified,
                hunks: Vec::new(),
            });
            continue;
        }

        let Some(file) = files.last_mut() else {
            // Content before any `diff --git` line; nothing to attach it to.
            continue;
        };

        // `--- `/`+++ ` file headers only appear before the first hunk of a
        // file; once hunks have started, lines with these prefixes are hunk
        // content (e.g. a removed line whose content starts with "-- ").
        if file.hunks.is_empty() {
            if let Some(rest) = line.strip_prefix("--- ") {
                file.old_path = parse_diff_path(rest);
                continue;
            }
            if let Some(rest) = line.strip_prefix("+++ ") {
                file.new_path = parse_diff_path(rest);
                continue;
            }
        }
        if line.starts_with("new file mode") {
            file.status = FileStatus::Added;
            continue;
        }
        if line.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("rename to ") {
            file.status = FileStatus::Renamed;
            file.new_path = Some(rest.to_string());
            continue;
        }
        if (line.starts_with("Binary files") && line.ends_with("differ"))
            || line.starts_with("GIT binary patch")
        {
            file.status = FileStatus::Binary;
            continue;
        }

        if let Some((old_start, old_count, new_start, new_count, section)) = parse_hunk_header(line)
        {
            old_no = old_start;
            new_no = new_start;
            file.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                section,
                lines: Vec::new(),
            });
            continue;
        }

        if line.starts_with('\\') {
            // "\ No newline at end of file" marker; not a content line.
            continue;
        }

        let Some(hunk) = file.hunks.last_mut() else {
            continue;
        };

        let mut chars = line.chars();
        let marker = chars.next();
        let content = chars.as_str().to_string();

        match marker {
            // A genuinely blank line in a hunk still counts as context;
            // `chars.next()` returns `None` for it rather than `Some(' ')`.
            Some(' ') | None => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Context,
                    content,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                });
                old_no += 1;
                new_no += 1;
            }
            Some('-') => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Remove,
                    content,
                    old_no: Some(old_no),
                    new_no: None,
                });
                old_no += 1;
            }
            Some('+') => {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Add,
                    content,
                    old_no: None,
                    new_no: Some(new_no),
                });
                new_no += 1;
            }
            Some(_) => {
                // Unrecognized marker character; not a valid diff content
                // line, so skip it rather than misclassifying it.
            }
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "\
diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,5 @@ header-one
 line1
-old2
+new2
+new3
 line4
@@ -10,3 +11,3 @@ header-two
 a
-b
+B
 c
";

    #[test]
    fn parses_modified_file_with_two_hunks() {
        let files = parse_unified_diff(MODIFIED);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.new_path.as_deref(), Some("src/app.ts"));
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.hunks.len(), 2);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 4, 1, 5)
        );
        assert_eq!(h.section, "header-one");
        // line numbering
        assert_eq!(h.lines[0].old_no, Some(1));
        assert_eq!(h.lines[0].new_no, Some(1));
        assert_eq!(h.lines[1].kind as u8, LineKind::Remove as u8);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[1].new_no, None);
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[3].new_no, Some(3));
        assert_eq!(h.lines[4].old_no, Some(3));
        assert_eq!(h.lines[4].new_no, Some(4));
    }

    const ADDED: &str = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
";

    #[test]
    fn parses_added_file() {
        let f = &parse_unified_diff(ADDED)[0];
        assert_eq!(f.status, FileStatus::Added);
        assert_eq!(f.old_path, None);
        assert_eq!(f.new_path.as_deref(), Some("new.txt"));
        assert_eq!(f.hunks[0].new_count, 2);
    }

    const DELETED: &str = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 4444444..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-now
";

    #[test]
    fn parses_deleted_file() {
        let f = &parse_unified_diff(DELETED)[0];
        assert_eq!(f.status, FileStatus::Deleted);
        assert_eq!(f.new_path, None);
        assert_eq!(f.hunks[0].new_count, 0);
        assert_eq!(f.hunks[0].lines[0].new_no, None);
    }

    const RENAME_NO_CHANGE: &str = "\
diff --git a/old_name.rs b/new_name.rs
similarity index 100%
rename from old_name.rs
rename to new_name.rs
";

    #[test]
    fn parses_pure_rename_without_hunks() {
        let f = &parse_unified_diff(RENAME_NO_CHANGE)[0];
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("old_name.rs"));
        assert_eq!(f.new_path.as_deref(), Some("new_name.rs"));
        assert!(f.hunks.is_empty());
    }

    const BINARY: &str = "\
diff --git a/logo.png b/logo.png
index 5555555..6666666 100644
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn parses_binary_file() {
        let f = &parse_unified_diff(BINARY)[0];
        assert_eq!(f.status, FileStatus::Binary);
        assert!(f.hunks.is_empty());
        assert_eq!(f.new_path.as_deref(), Some("logo.png"));
    }

    #[test]
    fn skips_no_newline_marker() {
        let d = "\
diff --git a/x b/x
index 1..2 100644
--- a/x
+++ b/x
@@ -1 +1 @@
-a
\\ No newline at end of file
+b
\\ No newline at end of file
";
        let f = &parse_unified_diff(d)[0];
        assert_eq!(f.hunks[0].lines.len(), 2);
    }

    #[test]
    fn multiple_files() {
        let combined = format!("{MODIFIED}{ADDED}");
        assert_eq!(parse_unified_diff(&combined).len(), 2);
    }

    #[test]
    fn header_like_content_lines_are_not_misparsed() {
        // A removed line whose content starts with "-- " renders as "--- ..."
        // and an added line whose content starts with "++ " renders as
        // "+++ ..." — these must be treated as hunk content, not file headers.
        let d = "\
diff --git a/query.sql b/query.sql
index 1111111..2222222 100644
--- a/query.sql
+++ b/query.sql
@@ -1,3 +1,3 @@
 SELECT 1;
--- old comment
+++ new comment
 SELECT 2;
";
        let files = parse_unified_diff(d);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.old_path.as_deref(), Some("query.sql"));
        assert_eq!(f.new_path.as_deref(), Some("query.sql"));
        let h = &f.hunks[0];
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind as u8, LineKind::Context as u8);
        assert_eq!(h.lines[1].kind as u8, LineKind::Remove as u8);
        assert_eq!(h.lines[1].content, "-- old comment");
        assert_eq!(h.lines[2].kind as u8, LineKind::Add as u8);
        assert_eq!(h.lines[2].content, "++ new comment");
        assert_eq!(h.lines[3].kind as u8, LineKind::Context as u8);
        assert_eq!(h.lines[3].old_no, Some(3));
        assert_eq!(h.lines[3].new_no, Some(3));
    }
}

#[derive(Debug)]
pub enum GitError {
    NotARepo,
    BadBase(String),
    GitFailed(String),
}

/// Repo root of cwd, or `NotARepo`. Uses `git rev-parse --show-toplevel`.
pub fn repo_root() -> Result<std::path::PathBuf, GitError> {
    let output = base_git()
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| GitError::NotARepo)?;
    if !output.status.success() {
        return Err(GitError::NotARepo);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(path))
}

/// Current branch name (for `--title` default). `git rev-parse --abbrev-ref HEAD`;
/// on any failure returns `"review"`.
pub fn current_branch(root: &std::path::Path) -> String {
    let output = git_cmd(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                "review".to_string()
            } else {
                branch
            }
        }
        _ => "review".to_string(),
    }
}

/// Largest single blob that will be rendered inline. Files with a bigger
/// blob on either side are reported as `FileStatus::TooLarge` with a warning.
pub const MAX_FILE_BYTES: usize = 1_048_576;

/// Total blob-content budget for one diff. Once exceeded, remaining files
/// are reported as `FileStatus::TooLarge` with a warning rather than being
/// silently truncated.
pub const MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;

/// Result of [`compute_diff`]: the per-file diffs plus non-fatal warnings
/// (e.g. files skipped because they exceed size limits).
#[derive(Debug)]
pub struct DiffOutput {
    pub files: Vec<FileDiff>,
    pub warnings: Vec<String>,
}

/// One record of `git diff-tree -r -z --raw` output.
struct RawEntry {
    old_mode: String,
    new_mode: String,
    old_oid: String,
    new_oid: String,
    status: char,
    path: String,
    path2: Option<String>,
}

/// How a raw entry will be rendered, decided before blob contents are fetched.
enum Plan {
    /// Gitlink (mode 160000) on either side: synthesize `Subproject commit`
    /// lines; never `cat-file` the oid (it usually doesn't exist locally).
    Submodule,
    /// Identical blob oids on both sides (pure rename or mode-only change):
    /// no hunks, no content needed.
    NoContent,
    /// Over a size limit: `FileStatus::TooLarge`, no hunks.
    TooLarge,
    /// Fetch both blobs and diff them.
    Content,
}

/// Environment variables that can redirect which repository/objects git
/// reads, alter diff output, or inject arbitrary git config. The reviewed
/// agent shares this process environment, so all of them are stripped
/// before running git.
const SCRUBBED_GIT_ENV: &[&str] = &[
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    // GIT_CONFIG_PARAMETERS can itself encode arbitrary key/value pairs
    // (including fake GIT_CONFIG_KEY_<n>/GIT_CONFIG_VALUE_<n> style
    // injection) that git decodes directly, independent of the count var.
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    // GIT_CONFIG_KEY_<n>/GIT_CONFIG_VALUE_<n> are dynamically named and
    // cannot be enumerated here, but git only reads them up to
    // GIT_CONFIG_COUNT, so scrubbing that count var alone neutralizes them.
];

/// Base `git` invocation: replacement refs disabled (an in-repo agent can
/// `git replace` HEAD with an innocent-looking commit), repo-redirection
/// env stripped, and `LC_ALL=C` for stable message parsing.
fn base_git() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("--no-replace-objects")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("LC_ALL", "C");
    for var in SCRUBBED_GIT_ENV {
        cmd.env_remove(var);
    }
    cmd
}

/// Base `git` invocation for this module, scoped to `root` via `-C`. See
/// [`base_git`] for the hardening this applies (replacement refs disabled,
/// repo-redirection env scrubbed). All commands used here are plumbing that
/// ignores diff drivers anyway, but removing them is cheap defense in depth.
fn git_cmd(root: &std::path::Path) -> std::process::Command {
    let mut cmd = base_git();
    cmd.arg("-C").arg(root);
    cmd
}

fn run_git(root: &std::path::Path, args: &[&str]) -> Result<std::process::Output, GitError> {
    git_cmd(root)
        .args(args)
        .output()
        .map_err(|e| GitError::GitFailed(e.to_string()))
}

/// Resolves `<rev>^{commit}` to a full oid. On failure returns the stderr
/// text (the caller decides whether that is `BadBase` or `GitFailed`).
fn rev_parse_commit(root: &std::path::Path, rev: &str) -> Result<String, String> {
    let output = git_cmd(root)
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_zero_oid(oid: &str) -> bool {
    !oid.is_empty() && oid.bytes().all(|b| b == b'0')
}

/// Parses `git diff-tree -r -z --raw` output:
/// `:<oldmode> <newmode> <oldoid> <newoid> <status>\0<path>\0[<path2>\0]`.
/// Paths are NUL-delimited so they arrive verbatim (no quoting/escaping).
/// Any structurally malformed record is a hard error: this parser feeds the
/// review gate, so partial success is worse than failing the whole diff.
fn parse_raw_z(bytes: &[u8]) -> Result<Vec<RawEntry>, GitError> {
    let malformed = |detail: &str| GitError::GitFailed(format!("unexpected diff-tree output: {detail}"));
    let mut tokens = bytes.split(|&b| b == 0);
    let mut entries = Vec::new();
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            // The trailing NUL leaves one empty token; anything after it
            // would be malformed and is caught on the next iteration.
            continue;
        }
        let meta = String::from_utf8_lossy(token).to_string();
        let Some(meta) = meta.strip_prefix(':') else {
            return Err(malformed(&format!("record does not start with ':': {meta:?}")));
        };
        let parts: Vec<&str> = meta.split(' ').collect();
        if parts.len() < 5 {
            return Err(malformed(&format!("record has {} fields (expected 5): {meta:?}", parts.len())));
        }
        let status = parts[4].chars().next().ok_or_else(|| malformed("empty status field"))?;
        let path_token = tokens
            .next()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| malformed(&format!("record {meta:?} has no path")))?;
        let path = String::from_utf8_lossy(path_token).to_string();
        let path2 = if matches!(status, 'R' | 'C') {
            let token = tokens
                .next()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| malformed(&format!("rename/copy record {meta:?} has no second path")))?;
            Some(String::from_utf8_lossy(token).to_string())
        } else {
            None
        };
        entries.push(RawEntry {
            old_mode: parts[0].to_string(),
            new_mode: parts[1].to_string(),
            old_oid: parts[2].to_string(),
            new_oid: parts[3].to_string(),
            status,
            path,
            path2,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod raw_tests {
    use super::*;

    #[test]
    fn parse_raw_z_valid_record() {
        let raw = b":100644 100755 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0a.txt\0";
        let entries = parse_raw_z(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].status, 'M');
    }

    #[test]
    fn parse_raw_z_rejects_garbage_meta() {
        // A token that isn't `:`-prefixed is not a diff-tree raw record;
        // silently skipping it would desynchronize the path tokens.
        assert!(parse_raw_z(b"garbage\0a.txt\0").is_err());
    }

    #[test]
    fn parse_raw_z_rejects_truncated_record() {
        // Meta token with no following path token.
        assert!(parse_raw_z(b":100644 100644 111 222 M\0").is_err());
        // Rename record missing its second path.
        assert!(parse_raw_z(
            b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R90\0old.txt\0"
        )
        .is_err());
        // Meta with fewer than 5 fields.
        assert!(parse_raw_z(b":100644 100644 M\0a.txt\0").is_err());
    }
}

fn is_submodule(entry: &RawEntry) -> bool {
    entry.old_mode == "160000" || entry.new_mode == "160000"
}

/// Path to show in user-facing warnings: the post-change path when present.
fn display_path(entry: &RawEntry) -> &str {
    entry.path2.as_deref().unwrap_or(&entry.path)
}

fn entry_paths(entry: &RawEntry) -> (Option<String>, Option<String>) {
    match entry.status {
        'A' => (None, Some(entry.path.clone())),
        'D' => (Some(entry.path.clone()), None),
        'R' | 'C' => (Some(entry.path.clone()), entry.path2.clone()),
        _ => (Some(entry.path.clone()), Some(entry.path.clone())),
    }
}

fn entry_status(entry: &RawEntry) -> FileStatus {
    match entry.status {
        'A' => FileStatus::Added,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

/// Runs `git cat-file <flag>` feeding `input` (one oid per line) on stdin
/// and returning raw stdout. Stdin is written from a helper thread so a
/// large response can't deadlock against a full stdin pipe.
fn cat_file_stdin(root: &std::path::Path, flag: &str, input: &str) -> Result<Vec<u8>, GitError> {
    use std::io::Write;
    let mut child = git_cmd(root)
        .args(["cat-file", flag])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| GitError::GitFailed(e.to_string()))?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let input = input.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let output = child
        .wait_with_output()
        .map_err(|e| GitError::GitFailed(e.to_string()))?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(GitError::GitFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(output.stdout)
}

/// Object sizes via `git cat-file --batch-check`. Oids reported `missing`
/// are simply absent from the map (callers treat that as an error).
fn blob_sizes(
    root: &std::path::Path,
    oids: &[String],
) -> Result<std::collections::HashMap<String, usize>, GitError> {
    let mut sizes = std::collections::HashMap::new();
    if oids.is_empty() {
        return Ok(sizes);
    }
    let mut input = oids.join("\n");
    input.push('\n');
    let out = cat_file_stdin(root, "--batch-check", &input)?;
    for line in String::from_utf8_lossy(&out).lines() {
        let parts: Vec<&str> = line.split(' ').collect();
        if parts.len() == 3 {
            if let Ok(size) = parts[2].parse() {
                sizes.insert(parts[0].to_string(), size);
            }
        }
    }
    Ok(sizes)
}

/// Blob contents via `git cat-file --batch`: for each requested oid the
/// response is `<oid> <type> <size>\n<content>\n`.
fn blob_contents(
    root: &std::path::Path,
    oids: &[String],
) -> Result<std::collections::HashMap<String, Vec<u8>>, GitError> {
    let mut contents = std::collections::HashMap::new();
    if oids.is_empty() {
        return Ok(contents);
    }
    let mut input = oids.join("\n");
    input.push('\n');
    let buf = cat_file_stdin(root, "--batch", &input)?;
    let mut pos = 0;
    while pos < buf.len() {
        let nl = buf[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| GitError::GitFailed("truncated cat-file --batch output".to_string()))?;
        let header = String::from_utf8_lossy(&buf[pos..pos + nl]).to_string();
        pos += nl + 1;
        let parts: Vec<&str> = header.split(' ').collect();
        if parts.len() < 3 {
            return Err(GitError::GitFailed(format!("git cat-file: {header}")));
        }
        let size: usize = parts[2]
            .parse()
            .map_err(|_| GitError::GitFailed(format!("git cat-file: {header}")))?;
        if pos + size > buf.len() {
            return Err(GitError::GitFailed(
                "truncated cat-file --batch output".to_string(),
            ));
        }
        contents.insert(parts[0].to_string(), buf[pos..pos + size].to_vec());
        pos += size + 1; // skip content and the trailing newline
    }
    Ok(contents)
}

fn blob_of<'a>(contents: &'a std::collections::HashMap<String, Vec<u8>>, oid: &str) -> &'a [u8] {
    if is_zero_oid(oid) {
        &[]
    } else {
        contents.get(oid).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Same heuristic git uses: a NUL byte within the first 8000 bytes.
fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// Synthesizes the `Subproject commit <oid>` hunk for a gitlink entry,
/// mirroring what `git diff` prints for submodule pointer changes.
fn submodule_diff(entry: &RawEntry) -> (FileStatus, Vec<Hunk>) {
    let mut lines = Vec::new();
    if entry.old_mode == "160000" && !is_zero_oid(&entry.old_oid) {
        lines.push(DiffLine {
            kind: LineKind::Remove,
            content: format!("Subproject commit {}", entry.old_oid),
            old_no: Some(1),
            new_no: None,
        });
    }
    if entry.new_mode == "160000" && !is_zero_oid(&entry.new_oid) {
        lines.push(DiffLine {
            kind: LineKind::Add,
            content: format!("Subproject commit {}", entry.new_oid),
            old_no: None,
            new_no: Some(1),
        });
    }
    let old_count = lines
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Remove))
        .count() as u32;
    let new_count = lines
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Add))
        .count() as u32;
    let hunk = Hunk {
        old_start: if old_count > 0 { 1 } else { 0 },
        old_count,
        new_start: if new_count > 0 { 1 } else { 0 },
        new_count,
        section: String::new(),
        lines,
    };
    (entry_status(entry), vec![hunk])
}

/// Line-based text diff with 3 lines of context, built directly from blob
/// contents (never from `git diff` porcelain output).
fn text_hunks(old: &str, new: &str) -> Vec<Hunk> {
    use similar::ChangeTag;
    let diff = similar::TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_count = (last.old_range().end - first.old_range().start) as u32;
        let new_count = (last.new_range().end - first.new_range().start) as u32;
        // Unified-diff convention: a zero-count side reports the line
        // *before* the change (0 when at the top of the file).
        let old_start = first.old_range().start as u32 + u32::from(old_count > 0);
        let new_start = first.new_range().start as u32 + u32::from(new_count > 0);
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let (kind, old_no, new_no) = match change.tag() {
                    ChangeTag::Equal => (
                        LineKind::Context,
                        change.old_index().map(|i| i as u32 + 1),
                        change.new_index().map(|i| i as u32 + 1),
                    ),
                    ChangeTag::Delete => (
                        LineKind::Remove,
                        change.old_index().map(|i| i as u32 + 1),
                        None,
                    ),
                    ChangeTag::Insert => (
                        LineKind::Add,
                        None,
                        change.new_index().map(|i| i as u32 + 1),
                    ),
                };
                let value = change.value();
                let content = value.strip_suffix('\n').unwrap_or(value);
                let content = content.strip_suffix('\r').unwrap_or(content);
                lines.push(DiffLine {
                    kind,
                    content: content.to_string(),
                    old_no,
                    new_no,
                });
            }
        }
        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            section: String::new(),
            lines,
        });
    }
    hunks
}

/// Computes the diff of `<base>...HEAD` (merge-base semantics) directly
/// from blob contents instead of parsing `git diff` text output.
///
/// This is deliberate: the reviewed agent shares the working environment,
/// and `git diff` porcelain output can be manipulated from inside the repo
/// via `.gitattributes` (`-diff` makes files render as "Binary files
/// differ"), textconv/external diff drivers (arbitrary fake content), and
/// config like `diff.noprefix` (breaks `a/`/`b/` parsing). This pipeline
/// only uses plumbing that reads object data:
///
/// 1. `rev-parse --verify` both endpoints (bad base -> `BadBase`),
/// 2. `merge-base` to reproduce `...` semantics,
/// 3. `diff-tree -r -z -M --full-index --raw` for the file list (NUL
///    delimited, so paths arrive verbatim regardless of `core.quotepath`),
/// 4. `cat-file --batch-check` / `--batch` for blob sizes and contents,
/// 5. an in-process line diff (`similar`) with 3 context lines.
///
/// None of these read `.gitattributes`, diff drivers, or diff config, so
/// what the reviewer sees is derived from the actual committed blobs.
pub fn compute_diff(root: &std::path::Path, base: &str) -> Result<DiffOutput, GitError> {
    let base_oid = rev_parse_commit(root, base).map_err(GitError::BadBase)?;
    let head_oid = rev_parse_commit(root, "HEAD").map_err(GitError::GitFailed)?;

    let out = run_git(root, &["merge-base", &base_oid, &head_oid])?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(GitError::GitFailed(format!(
            "no merge base between {base} and HEAD{}{stderr}",
            if stderr.is_empty() { "" } else { ": " }
        )));
    }
    let merge_base = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if merge_base.is_empty() {
        return Err(GitError::GitFailed(format!(
            "no merge base between {base} and HEAD"
        )));
    }

    let out = run_git(
        root,
        &[
            "diff-tree",
            "-r",
            "-z",
            "-M",
            "--full-index",
            "--raw",
            &merge_base,
            &head_oid,
        ],
    )?;
    if !out.status.success() {
        return Err(GitError::GitFailed(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    let entries = parse_raw_z(&out.stdout)?;

    // Sizes first (--batch-check), so oversized blobs are never ingested.
    let mut size_oids: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in &entries {
        if is_submodule(entry) || entry.old_oid == entry.new_oid {
            continue;
        }
        for oid in [&entry.old_oid, &entry.new_oid] {
            if !is_zero_oid(oid) && seen.insert(oid.clone()) {
                size_oids.push(oid.clone());
            }
        }
    }
    let sizes = blob_sizes(root, &size_oids)?;

    let mut warnings = Vec::new();
    let mut plans = Vec::with_capacity(entries.len());
    let mut need_oids: Vec<String> = Vec::new();
    let mut need_seen = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;
    for entry in &entries {
        if is_submodule(entry) {
            plans.push(Plan::Submodule);
            continue;
        }
        if entry.old_oid == entry.new_oid {
            // Pure rename (R100) or mode-only change: nothing to show.
            plans.push(Plan::NoContent);
            continue;
        }
        let mut file_bytes = 0usize;
        let mut max_blob = 0usize;
        for oid in [&entry.old_oid, &entry.new_oid] {
            if is_zero_oid(oid) {
                continue;
            }
            let Some(&size) = sizes.get(oid.as_str()) else {
                return Err(GitError::GitFailed(format!(
                    "object {oid} missing (file {})",
                    display_path(entry)
                )));
            };
            file_bytes += size;
            max_blob = max_blob.max(size);
        }
        if max_blob > MAX_FILE_BYTES {
            warnings.push(format!(
                "file too large to display inline: {} ({max_blob} bytes)",
                display_path(entry)
            ));
            plans.push(Plan::TooLarge);
            continue;
        }
        if total_bytes + file_bytes > MAX_TOTAL_BYTES {
            warnings.push(format!(
                "diff too large: {} not displayed",
                display_path(entry)
            ));
            plans.push(Plan::TooLarge);
            continue;
        }
        total_bytes += file_bytes;
        for oid in [&entry.old_oid, &entry.new_oid] {
            if !is_zero_oid(oid) && need_seen.insert(oid.clone()) {
                need_oids.push(oid.clone());
            }
        }
        plans.push(Plan::Content);
    }

    let contents = blob_contents(root, &need_oids)?;

    let mut files = Vec::with_capacity(entries.len());
    for (entry, plan) in entries.iter().zip(&plans) {
        let (old_path, new_path) = entry_paths(entry);
        let (status, hunks) = match plan {
            Plan::Submodule => submodule_diff(entry),
            Plan::NoContent => (entry_status(entry), Vec::new()),
            Plan::TooLarge => (FileStatus::TooLarge, Vec::new()),
            Plan::Content => {
                let old = blob_of(&contents, &entry.old_oid);
                let new = blob_of(&contents, &entry.new_oid);
                if is_binary(old) || is_binary(new) {
                    (FileStatus::Binary, Vec::new())
                } else {
                    match (std::str::from_utf8(old), std::str::from_utf8(new)) {
                        (Ok(old_text), Ok(new_text)) => {
                            (entry_status(entry), text_hunks(old_text, new_text))
                        }
                        _ => {
                            // Different byte contents can lossy-decode to the
                            // same string; never diff lossily.
                            warnings.push(format!(
                                "file content is not valid UTF-8, not rendered: {}",
                                display_path(entry)
                            ));
                            (FileStatus::NonUtf8, Vec::new())
                        }
                    }
                }
            }
        };
        files.push(FileDiff {
            old_path,
            new_path,
            status,
            hunks,
        });
    }

    Ok(DiffOutput { files, warnings })
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let st = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn fixture_repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        std::fs::write(d.join("b.txt"), "new file\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);
        td
    }

    /// Repo with just a base commit on `main` and a `feature` branch
    /// checked out, ready for per-test changes.
    fn base_repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        td
    }

    fn find<'a>(files: &'a [FileDiff], new_path: &str) -> &'a FileDiff {
        files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(new_path))
            .unwrap_or_else(|| panic!("no file with new_path {new_path}"))
    }

    fn hunk_contents(f: &FileDiff) -> Vec<&str> {
        f.hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.content.as_str()))
            .collect()
    }

    /// Runs git in `dir` and returns trimmed stdout (asserting success).
    fn git_stdout(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn replace_refs_cannot_hide_changes() {
        // An agent can `git replace` the real HEAD with a fake commit whose
        // tree equals the base tree; every git command then silently reads
        // the fake commit and the diff comes back empty. --no-replace-objects
        // must defeat this.
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nEVIL\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "evil change"]);
        let head = git_stdout(d, &["rev-parse", "HEAD"]);
        let base_tree = git_stdout(d, &["rev-parse", "main^{tree}"]);
        let fake = git_stdout(d, &["commit-tree", &base_tree, "-p", "main", "-m", "innocent"]);
        git(d, &["replace", &head, &fake]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.status, FileStatus::Modified);
        assert!(hunk_contents(a).contains(&"EVIL"));
    }

    #[test]
    fn computes_diff_against_base() {
        let td = fixture_repo();
        let out = compute_diff(td.path(), "main").unwrap();
        assert!(out.warnings.is_empty());
        assert_eq!(out.files.len(), 2);
        let a = find(&out.files, "a.txt");
        assert_eq!(a.status, FileStatus::Modified);
        let h = &a.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 3, 1, 4)
        );
        let contents = hunk_contents(a);
        assert!(contents.contains(&"TWO"));
        assert!(contents.contains(&"four"));
        // line numbering: "one" is context 1/1, "two" removed at old 2,
        // "TWO" added at new 2.
        assert_eq!(h.lines[0].old_no, Some(1));
        assert_eq!(h.lines[0].new_no, Some(1));
        let removed = h
            .lines
            .iter()
            .find(|l| matches!(l.kind, LineKind::Remove))
            .unwrap();
        assert_eq!((removed.old_no, removed.new_no), (Some(2), None));
        let b = find(&out.files, "b.txt");
        assert_eq!(b.status, FileStatus::Added);
        assert_eq!(b.old_path, None);
        assert_eq!(b.hunks[0].old_start, 0);
        assert_eq!(b.hunks[0].old_count, 0);
        assert_eq!(b.hunks[0].new_start, 1);
        assert_eq!(b.hunks[0].new_count, 1);
    }

    #[test]
    fn bad_base_is_distinguished() {
        let td = fixture_repo();
        assert!(matches!(
            compute_diff(td.path(), "no-such-ref"),
            Err(GitError::BadBase(_))
        ));
    }

    #[test]
    fn empty_diff_returns_empty_vec() {
        let td = fixture_repo();
        let out = compute_diff(td.path(), "HEAD").unwrap();
        assert!(out.files.is_empty());
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn current_branch_name() {
        let td = fixture_repo();
        assert_eq!(current_branch(td.path()), "feature");
    }

    #[test]
    fn non_ascii_path_is_verbatim() {
        // The raw entries are NUL-delimited (`diff-tree -z`), so non-ASCII
        // paths must arrive verbatim regardless of `core.quotepath`.
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("日本語.txt"), "hello\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "add japanese file"]);

        let out = compute_diff(d, "main").unwrap();
        let f = out
            .files
            .iter()
            .find(|f| f.status == FileStatus::Added)
            .expect("added file present");
        assert_eq!(f.new_path.as_deref(), Some("日本語.txt"));
    }

    #[test]
    fn gitattributes_no_diff_cannot_hide_content() {
        // An agent marking files `-diff` via a committed .gitattributes
        // must not turn text changes into "Binary files differ".
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join(".gitattributes"), "*.txt -diff\n").unwrap();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "sneaky"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.status, FileStatus::Modified);
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn worktree_gitattributes_cannot_hide_content() {
        // Same attack via an uncommitted .gitattributes in the working tree
        // (git's diff machinery reads attributes from the worktree).
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);
        std::fs::write(d.join(".gitattributes"), "*.txt -diff\n").unwrap();

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.status, FileStatus::Modified);
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn textconv_driver_cannot_fake_content() {
        // A repo-local textconv driver would let `git diff` display
        // arbitrary fake content; the blob-based diff must show the real
        // committed bytes.
        let td = base_repo();
        let d = td.path();
        git(d, &["config", "diff.x.textconv", "printf FAKE #"]);
        std::fs::write(d.join(".gitattributes"), "*.txt diff=x\n").unwrap();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "sneaky"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        let contents = hunk_contents(a);
        assert!(contents.contains(&"TWO"));
        assert!(!contents.iter().any(|c| c.contains("FAKE")));
    }

    #[test]
    fn external_diff_env_var_has_no_effect() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        // Would make `git diff` invoke a bogus external command; compute_diff
        // strips it (and uses plumbing that ignores it anyway).
        std::env::set_var("GIT_EXTERNAL_DIFF", "/nonexistent/fake-diff");
        let result = compute_diff(d, "main");
        std::env::remove_var("GIT_EXTERNAL_DIFF");
        let out = result.unwrap();
        let a = find(&out.files, "a.txt");
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn noprefix_config_cannot_break_paths() {
        // `diff.noprefix=true` breaks parsers expecting `a/`/`b/` prefixes;
        // the raw pipeline never sees prefixes at all.
        let td = base_repo();
        let d = td.path();
        git(d, &["config", "diff.noprefix", "true"]);
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.old_path.as_deref(), Some("a.txt"));
        assert_eq!(a.new_path.as_deref(), Some("a.txt"));
        assert!(!a.hunks.is_empty());
    }

    #[test]
    fn pure_rename_has_no_hunks() {
        let td = base_repo();
        let d = td.path();
        git(d, &["mv", "a.txt", "renamed.txt"]);
        git(d, &["commit", "-m", "rename"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        let f = &out.files[0];
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("renamed.txt"));
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn rename_with_modification_has_hunks() {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(d.join("a.txt"), &body).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        git(d, &["mv", "a.txt", "moved.txt"]);
        std::fs::write(d.join("moved.txt"), body.replace("line5", "LINE5")).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "rename and edit"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        let f = &out.files[0];
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("moved.txt"));
        let contents = hunk_contents(f);
        assert!(contents.contains(&"LINE5"));
    }

    #[test]
    fn submodule_pointer_change_is_visible() {
        let td = base_repo();
        let d = td.path();
        let sha = "1111111111111111111111111111111111111111";
        git(
            d,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},sub"),
            ],
        );
        git(d, &["commit", "-m", "add submodule pointer"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "sub");
        assert_eq!(f.status, FileStatus::Added);
        assert_eq!(f.hunks.len(), 1);
        let lines = &f.hunks[0].lines;
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0].kind, LineKind::Add));
        assert_eq!(lines[0].content, format!("Subproject commit {sha}"));
        assert_eq!(lines[0].new_no, Some(1));
    }

    #[test]
    fn nul_bytes_are_binary() {
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("blob.bin"), b"\x00\x01\x02text").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "binary"]);

        let out = compute_diff(d, "main").unwrap();
        let f = find(&out.files, "blob.bin");
        assert_eq!(f.status, FileStatus::Binary);
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn non_utf8_blobs_are_not_lossy_diffed() {
        // 0x80 -> 0x81: neither contains NUL, both are invalid UTF-8, and
        // both lossy-decode to "\u{FFFD}\n" — a lossy text diff would claim
        // "no content changes" for a real byte-level change.
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("data.txt"), [0x80, b'\n']).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        std::fs::write(d.join("data.txt"), [0x81, b'\n']).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.files[0].status, FileStatus::NonUtf8);
        assert!(out.files[0].hunks.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("data.txt"),
            "warning should name the file: {}",
            out.warnings[0]
        );
    }

    #[test]
    fn oversized_file_is_too_large_with_warning() {
        let td = base_repo();
        let d = td.path();
        let big = "x".repeat(MAX_FILE_BYTES + 1);
        std::fs::write(d.join("big.txt"), &big).unwrap();
        std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "big"]);

        let out = compute_diff(d, "main").unwrap();
        let big_file = find(&out.files, "big.txt");
        assert_eq!(big_file.status, FileStatus::TooLarge);
        assert!(big_file.hunks.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("file too large to display inline: big.txt"),
            "unexpected warning: {}",
            out.warnings[0]
        );
        // Other files in the same diff are unaffected.
        let a = find(&out.files, "a.txt");
        assert!(hunk_contents(a).contains(&"TWO"));
    }

    #[test]
    fn three_dot_semantics_use_merge_base() {
        // Commits on the base branch after the fork point must not appear
        // in the diff (`<base>...HEAD` semantics, not `<base>..HEAD`).
        let td = fixture_repo();
        let d = td.path();
        git(d, &["checkout", "main"]);
        std::fs::write(d.join("main_only.txt"), "main moved on\n").unwrap();
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\nmain edit\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "main advances"]);
        git(d, &["checkout", "feature"]);

        let out = compute_diff(d, "main").unwrap();
        assert!(
            !out.files
                .iter()
                .any(|f| f.new_path.as_deref() == Some("main_only.txt")),
            "base-side commit leaked into the diff"
        );
        // The feature-side change is still reported relative to the fork.
        let a = find(&out.files, "a.txt");
        let contents = hunk_contents(a);
        assert!(contents.contains(&"TWO"));
        assert!(!contents.iter().any(|c| c.contains("main edit")));
    }
}
