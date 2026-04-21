use std::path::PathBuf;
use std::process::Command;

use crate::types::PromptRef;

pub struct PromptRegistry {
    prompts_dir: PathBuf,
}

impl PromptRegistry {
    pub fn new(prompts_dir: PathBuf) -> Self {
        Self { prompts_dir }
    }

    pub fn resolve(&self, name: &str) -> anyhow::Result<PromptRef> {
        let filename = format!("{name}.md");
        let path = self.prompts_dir.join(&filename);

        if !path.exists() {
            anyhow::bail!("prompt '{}' not found at {}", name, path.display());
        }

        let content = std::fs::read_to_string(&path)?;
        let git_sha = git_file_sha(&path);

        Ok(PromptRef {
            name: name.to_string(),
            content,
            git_sha,
        })
    }

    pub fn list(&self) -> anyhow::Result<Vec<String>> {
        let mut names = Vec::new();
        if self.prompts_dir.exists() {
            for entry in std::fs::read_dir(&self.prompts_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                {
                    names.push(stem.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }
}

fn git_file_sha(path: &std::path::Path) -> Option<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%H", "--"])
        .arg(path)
        .output()
        .ok()?;

    if output.status.success() {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if sha.is_empty() { None } else { Some(sha) }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_registry_with_prompts(prompts: &[(&str, &str)]) -> (PromptRegistry, TempDir) {
        let tmp = TempDir::new().unwrap();
        for (name, content) in prompts {
            let path = tmp.path().join(format!("{name}.md"));
            std::fs::write(path, content).unwrap();
        }
        let reg = PromptRegistry::new(tmp.path().to_path_buf());
        (reg, tmp)
    }

    #[test]
    fn resolve_returns_prompt_content() {
        let (reg, _tmp) = make_registry_with_prompts(&[("research", "Extract claims...")]);
        let prompt = reg.resolve("research").unwrap();
        assert_eq!(prompt.name, "research");
        assert_eq!(prompt.content, "Extract claims...");
        // git_sha is None in a tempdir (not a git repo) — graceful fallback
        assert!(prompt.git_sha.is_none());
    }

    #[test]
    fn resolve_missing_prompt_errors() {
        let (reg, _tmp) = make_registry_with_prompts(&[]);
        let result = reg.resolve("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not found"));
    }

    #[test]
    fn list_returns_sorted_prompt_names() {
        let (reg, _tmp) = make_registry_with_prompts(&[
            ("research", "r"),
            ("personal", "p"),
            ("investment", "i"),
        ]);
        let names = reg.list().unwrap();
        assert_eq!(names, vec!["investment", "personal", "research"]);
    }

    #[test]
    fn list_ignores_non_md_files() {
        let (reg, tmp) = make_registry_with_prompts(&[("research", "content")]);
        // Add a non-.md file
        std::fs::write(tmp.path().join("notes.txt"), "not a prompt").unwrap();
        std::fs::write(tmp.path().join("config.toml"), "not a prompt").unwrap();

        let names = reg.list().unwrap();
        assert_eq!(names, vec!["research"]);
    }

    #[test]
    fn list_empty_directory() {
        let (reg, _tmp) = make_registry_with_prompts(&[]);
        let names = reg.list().unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn list_nonexistent_directory() {
        let reg = PromptRegistry::new(PathBuf::from("/tmp/nonexistent-prompts-dir-xyz"));
        let names = reg.list().unwrap();
        assert!(names.is_empty());
    }
}
