use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskConfig {
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    #[serde(default)]
    pub prompt: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub fn tasks_file_path(cwd: &Path) -> PathBuf {
    cwd.join(".codex").join("tasks.yaml")
}

pub fn init_tasks_file(cwd: &Path) -> Result<PathBuf> {
    let path = tasks_file_path(cwd);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating directory {}", dir.display()))?;
    }
    let cfg = TaskConfig { tasks: Vec::new() };
    let yaml = serde_yaml::to_string(&cfg).context("serializing empty tasks config")?;
    fs::write(&path, yaml).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn load_tasks(cwd: &Path) -> Result<TaskConfig> {
    let path = tasks_file_path(cwd);
    if !path.exists() {
        return Ok(TaskConfig::default());
    }
    let s = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: TaskConfig =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save_tasks(cwd: &Path, cfg: &TaskConfig) -> Result<()> {
    let path = tasks_file_path(cwd);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating directory {}", dir.display()))?;
    }
    let yaml = serde_yaml::to_string(cfg).context("serializing tasks config")?;
    fs::write(&path, yaml).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn add_or_update_task(
    cwd: &Path,
    name: &str,
    prompt: String,
    description: Option<String>,
) -> Result<()> {
    let mut cfg = load_tasks(cwd)?;
    let prompt_lines: Vec<String> = prompt.lines().map(|s| s.to_string()).collect();
    if let Some(existing) = cfg.tasks.iter_mut().find(|t| t.name == name) {
        existing.prompt = prompt_lines;
        existing.prompt_file = None;
        existing.description = description;
    } else {
        cfg.tasks.push(Task {
            name: name.to_string(),
            prompt: prompt_lines,
            prompt_file: None,
            description,
        });
    }
    save_tasks(cwd, &cfg)
}

/// Add or update a task to reference a prompt file stored under `.codex/`.
/// If the file does not exist, it will be created (empty).
pub fn add_or_update_task_file(
    cwd: &Path,
    name: &str,
    rel_path: &str,
    description: Option<String>,
) -> Result<PathBuf> {
    let mut cfg = load_tasks(cwd)?;
    let codex_dir = cwd.join(".codex");
    fs::create_dir_all(&codex_dir)
        .with_context(|| format!("creating directory {}", codex_dir.display()))?;
    // Only allow relative paths to avoid escaping the workspace codex dir.
    let rel = Path::new(rel_path);
    if rel.is_absolute() {
        return Err(anyhow::anyhow!(
            "prompt file path must be relative to .codex"
        ));
    }
    let abs_path = codex_dir.join(rel);
    if let Some(parent) = abs_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    if !abs_path.exists() {
        fs::write(&abs_path, "").with_context(|| format!("creating {}", abs_path.display()))?;
    }
    let rel_str = rel.to_string_lossy().to_string();
    if let Some(existing) = cfg.tasks.iter_mut().find(|t| t.name == name) {
        existing.prompt.clear();
        existing.prompt_file = Some(rel_str.clone());
        if description.is_some() {
            existing.description = description.clone();
        }
    } else {
        cfg.tasks.push(Task {
            name: name.to_string(),
            prompt: Vec::new(),
            prompt_file: Some(rel_str.clone()),
            description,
        });
    }
    save_tasks(cwd, &cfg)?;
    Ok(abs_path)
}

pub fn list_task_names(cwd: &Path) -> Result<Vec<String>> {
    let cfg = load_tasks(cwd)?;
    Ok(cfg.tasks.into_iter().map(|t| t.name).collect())
}

pub fn get_task_prompt(cwd: &Path, name: &str) -> Result<Option<String>> {
    let cfg = load_tasks(cwd)?;
    let task_opt = cfg.tasks.into_iter().find(|t| t.name == name);
    if let Some(t) = task_opt {
        if let Some(file_rel) = t.prompt_file {
            let path = cwd.join(".codex").join(file_rel);
            let s =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            return Ok(Some(s));
        }
        return Ok(Some(t.prompt.join("\n")));
    }
    Ok(None)
}
