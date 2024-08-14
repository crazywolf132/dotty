use anyhow::{Context, Result};
use chrono::prelude::*;
use clap::Parser;
use colored::*;
use git2::{Cred, RemoteCallbacks, Repository};
use ignore::WalkBuilder;
use job_scheduler::{Job, JobScheduler};
use log::{error, info, warn};
use notify::{watcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, SystemTime};
use std::{env, fs};
use symlink::symlink_file;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    Add {
        #[clap(value_parser = clap::value_parser!(PathBuf))]
        path: PathBuf,
        #[clap(short, long)]
        profile: Option<String>,
    },
    Remove {
        #[clap(value_parser = clap::value_parser!(PathBuf))]
        path: PathBuf,
        #[clap(short, long)]
        profile: Option<String>,
    },
    Sync {
        #[clap(short, long)]
        profile: Option<String>,
    },
    Watch {
        #[clap(short, long)]
        profile: Option<String>,
    },
    Schedule {
        #[clap(short, long)]
        interval: u64,
        #[clap(short, long)]
        profile: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Clone)]
struct RemoteConfig {
    github_repo: String,
    github_token: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct ProfileConfig {
    files: HashMap<String, String>,
    ignore_patterns: Vec<String>,
    use_symlinks: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    profiles: HashMap<String, ProfileConfig>,
    remote: RemoteConfig,
    sync_interval: u64,
    profile_detection: Option<ProfileDetectionConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ProfileDetectionConfig {
    rules: Vec<ProfileDetectionRule>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ProfileDetectionRule {
    profile: String,
    conditions: Vec<DetectionCondition>,
}

#[derive(Serialize, Deserialize, Clone)]
enum DetectionCondition {
    Hostname(String),
    OS(String),
    EnvVar { name: String, value: String },
}

impl Config {
    fn validate(&self) -> Result<()> {
        if self.remote.github_repo.is_empty() {
            anyhow::bail!("GitHub repository URL is missing in the configuration");
        }
        if self.remote.github_token.is_empty() {
            anyhow::bail!("GitHub token is missing in the configuration");
        }
        if self.sync_interval == 0 {
            anyhow::bail!("Sync interval must be greater than 0");
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Dotty {
    config: Config,
    config_path: PathBuf,
    current_profile: String,
    last_synced: SystemTime,
}

impl Dotty {
    fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .context("Failed to get config directory")?
            .join("dotty");
        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
        let config_path = config_dir.join("config.toml");

        let config = if config_path.exists() {
            let config_str =
                fs::read_to_string(&config_path).context("Failed to read config file")?;
            let config: Config =
                toml::from_str(&config_str).context("Failed to parse config file")?;
            config.validate()?;
            config
        } else {
            let default_config = Config {
                profiles: HashMap::from([(
                    "default".to_string(),
                    ProfileConfig {
                        files: HashMap::new(),
                        ignore_patterns: vec![".git".to_string(), ".gitignore".to_string()],
                        use_symlinks: false,
                    },
                )]),
                remote: RemoteConfig {
                    github_repo: String::new(),
                    github_token: String::new(),
                },
                sync_interval: 300,
                profile_detection: None,
            };
            let config_str = toml::to_string_pretty(&default_config)
                .context("Failed to serialize default config")?;
            fs::write(&config_path, config_str).context("Failed to write default config file")?;
            default_config
        };

        let mut dotty = Dotty {
            config,
            config_path,
            current_profile: String::new(), // We'll set this in a moment
            last_synced: SystemTime::now(),
        };

        // Set the current profile based on automatic detection
        dotty.current_profile = dotty.detect_profile();

        Ok(dotty)
    }

    fn detect_profile(&self) -> String {
        if let Some(profile_detection) = &self.config.profile_detection {
            for rule in &profile_detection.rules {
                if rule
                    .conditions
                    .iter()
                    .all(|condition| self.check_condition(condition))
                {
                    return rule.profile.clone();
                }
            }
        }
        "default".to_string()
    }

    fn check_condition(&self, condition: &DetectionCondition) -> bool {
        match condition {
            DetectionCondition::Hostname(hostname) => {
                hostname == &hostname::get().unwrap().to_string_lossy()
            }
            DetectionCondition::OS(os) => os == env::consts::OS,
            DetectionCondition::EnvVar { name, value } => {
                env::var(name).map(|v| v == *value).unwrap_or(false)
            }
        }
    }

    fn save_config(&self) -> Result<()> {
        let config_str =
            toml::to_string_pretty(&self.config).context("Failed to serialize config")?;
        fs::write(&self.config_path, config_str).context("Failed to write config file")?;
        Ok(())
    }

    fn add_file(&mut self, path: &Path, profile: Option<String>) -> Result<()> {
        let profile = profile.unwrap_or_else(|| self.current_profile.clone());
        let profile_config = self
            .config
            .profiles
            .get_mut(&profile)
            .context("Profile not found")?;

        let canonical_path = path.canonicalize().context("Failed to canonicalize path")?;
        let relative_path = canonical_path
            .strip_prefix(dirs::home_dir().context("Failed to get home directory")?)
            .context("Path is not in home directory")?;
        profile_config.files.insert(
            relative_path.to_string_lossy().into_owned(),
            canonical_path.to_string_lossy().into_owned(),
        );
        self.save_config()?;
        info!("Added file: {:?} to profile {}", relative_path, profile);
        Ok(())
    }

    fn remove_file(&mut self, path: &Path, profile: Option<String>) -> Result<()> {
        let profile = profile.unwrap_or_else(|| self.current_profile.clone());
        let profile_config = self
            .config
            .profiles
            .get_mut(&profile)
            .context("Profile not found")?;

        let canonical_path = path.canonicalize().context("Failed to canonicalize path")?;
        let relative_path = canonical_path
            .strip_prefix(dirs::home_dir().context("Failed to get home directory")?)
            .context("Path is not in home directory")?;
        if profile_config
            .files
            .remove(&relative_path.to_string_lossy().into_owned())
            .is_some()
        {
            self.save_config()?;
            info!("Removed file: {:?} from profile {}", relative_path, profile);
        } else {
            warn!("File not found in config: {:?}", relative_path);
        }
        Ok(())
    }

    fn sync(&mut self, profile: Option<String>) -> Result<()> {
        let profile = profile.unwrap_or_else(|| self.current_profile.clone());
        let profile_config = self
            .config
            .profiles
            .get(&profile)
            .context("Profile not found")?;

        self.show_diff(&profile)?;

        for (relative_path, canonical_path) in &profile_config.files {
            let source = Path::new(canonical_path);
            let dest = dirs::home_dir()
                .context("Failed to get home directory")?
                .join(relative_path);

            if source.exists() {
                if self.should_sync(source, profile_config) {
                    self.backup_file(&dest)?;
                    if profile_config.use_symlinks {
                        symlink_file(source, &dest).context("Failed to create symlink")?;
                        info!("Created symlink: {:?} -> {:?}", dest, source);
                    } else {
                        fs::copy(source, &dest).context("Failed to copy file")?;
                        self.sync_permissions(source, &dest)?;
                        info!("Synced: {:?}", relative_path);
                    }
                } else {
                    info!("Skipped syncing {:?} (ignored)", relative_path);
                }
            } else {
                warn!("Source file missing: {:?}", canonical_path);
            }
        }

        self.sync_with_github()?;
        self.last_synced = SystemTime::now();
        Ok(())
    }

    fn show_diff(&self, profile: &str) -> Result<()> {
        let profile_config = self
            .config
            .profiles
            .get(profile)
            .context("Profile not found")?;

        for (relative_path, canonical_path) in &profile_config.files {
            let source = Path::new(canonical_path);
            let dest = dirs::home_dir()
                .context("Failed to get home directory")?
                .join(relative_path);

            if source.exists() && dest.exists() {
                let source_content =
                    fs::read_to_string(source).context("Failed to read source file")?;
                let dest_content =
                    fs::read_to_string(&dest).context("Failed to read destination file")?;

                let diff = TextDiff::from_lines(&dest_content, &source_content);

                println!("Diff for {}:", relative_path);
                for change in diff.iter_all_changes() {
                    let (sign, color) = match change.tag() {
                        ChangeTag::Delete => ("-", Color::Red),
                        ChangeTag::Insert => ("+", Color::Green),
                        ChangeTag::Equal => (" ", Color::White),
                    };
                    print!("{}", sign.color(color));
                    print!("{}", change.value().color(color));
                }
                println!();
            }
        }

        Ok(())
    }

    fn backup_file(&self, path: &Path) -> Result<()> {
        if path.exists() {
            let backup_path = path.with_extension("bak");
            fs::copy(path, &backup_path).context("Failed to create backup")?;
            info!("Created backup: {:?}", backup_path);
        }
        Ok(())
    }

    fn sync_permissions(&self, source: &Path, dest: &Path) -> Result<()> {
        let metadata = fs::metadata(source).context("Failed to get source file metadata")?;
        let permissions = metadata.permissions();
        fs::set_permissions(dest, permissions)
            .context("Failed to set permissions on destination file")?;
        Ok(())
    }

    fn should_sync(&self, path: &Path, profile_config: &ProfileConfig) -> bool {
        let walker = WalkBuilder::new(path)
            .hidden(false)
            .git_ignore(true)
            .build();

        for result in walker {
            match result {
                Ok(entry) => {
                    let path = entry.path();
                    if profile_config
                        .ignore_patterns
                        .iter()
                        .any(|pattern| path.to_str().map_or(false, |s| s.contains(pattern)))
                    {
                        return false;
                    }
                }
                Err(e) => {
                    warn!("Error walking directory: {}", e);
                }
            }
        }

        true
    }

    fn sync_with_github(&self) -> Result<()> {
        let repo_path = dirs::home_dir()
            .context("Failed to get home directory")?
            .join(".dotty_repo");

        let repo = if repo_path.exists() {
            Repository::open(&repo_path).context("Failed to open existing repository")?
        } else {
            Repository::clone(&self.config.remote.github_repo, &repo_path)
                .context("Failed to clone repository")?
        };

        // Copy files to the repo
        for profile_config in self.config.profiles.values() {
            for (relative_path, canonical_path) in &profile_config.files {
                let source = Path::new(canonical_path);
                let dest = repo_path.join(relative_path);

                if source.exists() {
                    fs::create_dir_all(dest.parent().unwrap())
                        .context("Failed to create parent directories")?;
                    fs::copy(source, &dest).context("Failed to copy file to repo")?;
                }
            }
        }

        // Commit and push changes
        let mut index = repo.index().context("Failed to get repo index")?;
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .context("Failed to add files to index")?;
        index.write().context("Failed to write index")?;

        let tree_id = index.write_tree().context("Failed to write tree")?;
        let tree = repo.find_tree(tree_id).context("Failed to find tree")?;

        let signature = repo.signature().context("Failed to get signature")?;
        let parent_commit = repo
            .head()
            .context("Failed to get HEAD")?
            .peel_to_commit()
            .context("Failed to peel to commit")?;

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "Sync dotfiles",
            &tree,
            &[&parent_commit],
        )
        .context("Failed to create commit")?;

        let mut remote = repo
            .find_remote("origin")
            .context("Failed to find remote 'origin'")?;
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|_, _, _| {
            Cred::userpass_plaintext("x-access-token", &self.config.remote.github_token)
        });

        remote
            .push(
                &["refs/heads/master:refs/heads/master"],
                Some(git2::PushOptions::new().remote_callbacks(callbacks)),
            )
            .context("Failed to push changes")?;

        info!("Synced with GitHub repository");
        Ok(())
    }

    fn watch_and_sync(&mut self, profile: Option<String>) -> Result<()> {
        let profile = profile.unwrap_or_else(|| self.current_profile.clone());
        let profile_config = self
            .config
            .profiles
            .get(&profile)
            .context("Profile not found")?;

        let (tx, rx) = channel();
        let mut watcher =
            watcher(tx, Duration::from_secs(1)).context("Failed to create watcher")?;

        for path in profile_config.files.values() {
            watcher
                .watch(path, RecursiveMode::NonRecursive)
                .context("Failed to watch path")?;
        }

        info!(
            "Watching for changes in profile {}. Press Ctrl-C to stop.",
            profile
        );

        loop {
            match rx.recv() {
                Ok(_event) => {
                    info!("Change detected, syncing...");
                    if let Err(e) = self.sync(Some(profile.clone())) {
                        error!("Error during sync: {}", e);
                    }
                }
                Err(e) => error!("Watch error: {:?}", e),
            }
        }
    }

    fn schedule_sync(&self, interval: u64, profile: Option<String>) -> Result<()> {
        let mut scheduler = JobScheduler::new();
        let profile = profile.unwrap_or_else(|| self.current_profile.clone());

        let profile_clone = profile.clone();
        scheduler.add(Job::new(
            format!("1/{} * * * * *", interval).parse().unwrap(),
            move || {
                let mut dotty = Dotty::new().expect("Failed to create Dotty instance");
                if let Err(e) = dotty.sync(Some(profile_clone.clone())) {
                    error!("Scheduled sync error: {}", e);
                }
            },
        ));

        info!(
            "Scheduled sync every {} minutes for profile {}",
            interval, profile
        );
        loop {
            scheduler.tick();
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();
    let mut dotty = Dotty::new()?;

    match args.command {
        Command::Add { path, profile } => dotty.add_file(&path, profile)?,
        Command::Remove { path, profile } => dotty.remove_file(&path, profile)?,
        Command::Sync { profile } => dotty.sync(profile)?,
        Command::Watch { profile } => dotty.watch_and_sync(profile)?,
        Command::Schedule { interval, profile } => dotty.schedule_sync(interval, profile)?,
    }

    Ok(())
}
