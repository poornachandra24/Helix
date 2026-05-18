use anyhow::Result;
use std::fs;
use std::path::PathBuf;

pub struct SkillRegistry {
    skills_dir: PathBuf,
}

impl SkillRegistry {
    pub fn new(skills_dir: PathBuf) -> Result<Self> {
        if !skills_dir.exists() {
            fs::create_dir_all(&skills_dir)?;
        }
        Ok(Self { skills_dir })
    }

    /// Load all `.txt` / `.md` files from the skills directory into a single
    /// prompt block. Returns `None` if no skills exist (avoids injecting an
    /// empty header into the system prompt).
    pub fn load_skills_prompt(&self) -> Option<String> {
        let mut sections: Vec<String> = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.skills_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
                if path.is_file() && matches!(ext, "txt" | "md") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if !content.trim().is_empty() {
                            sections.push(format!(
                                "--- Skill: {} ---\n{}",
                                path.file_name().unwrap_or_default().to_string_lossy(),
                                content.trim()
                            ));
                        }
                    }
                }
            }
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!(
                "\n\nYou have the following domain-specific skills available:\n{}",
                sections.join("\n\n")
            ))
        }
    }
}
