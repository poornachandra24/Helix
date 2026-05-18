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

    pub fn load_skills_prompt(&self) -> Result<String> {
        let mut prompt = String::from("\nYou have the following skills available (learned from previous sessions):\n");
        let mut has_skills = false;
        
        for entry in fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            if entry.path().is_file() {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    has_skills = true;
                    prompt.push_str(&format!("--- Skill: {:?} ---\n{}\n", entry.file_name(), content));
                }
            }
        }
        
        if has_skills {
            Ok(prompt)
        } else {
            Ok(String::new())
        }
    }
}
