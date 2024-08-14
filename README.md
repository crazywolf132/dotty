# Dotty: Your Dotfile Synchronization Solution

## 🌟 Introduction

Dotty is a powerful, flexible, and user-friendly dotfile synchronization tool written in Rust. It allows you to effortlessly manage and sync your configuration files across multiple devices, ensuring a consistent development environment wherever you go.

## 🚀 Features

- **Multi-profile Support**: Manage different sets of dotfiles for various environments or machines.
- **GitHub Integration**: Seamlessly sync your dotfiles with a GitHub repository.
- **Automatic Profile Detection**: Intelligently select the appropriate profile based on hostname, OS, or environment variables.
- **File Watching**: Automatically sync changes as soon as they occur.
- **Scheduled Syncing**: Set up periodic syncs to ensure your dotfiles are always up-to-date.
- **Symlink Support**: Choose between copying files or creating symlinks.
- **Backup Creation**: Automatically create backups before overwriting existing files.
- **Diff Viewing**: See the differences between local and synced files before applying changes.

## 🛠 Installation

To install Dotty, you'll need Rust and Cargo installed on your system. Then, you can build and install Dotty using:

```bash
git clone https://github.com/crazywolf132/dotty.git
cd dotty
cargo install --path .
```

## 📋 Usage

Here are some common commands to get you started with Dotty:

```bash
# Add a file to be managed by Dotty
dotty add /path/to/your/dotfile

# Remove a file from Dotty management
dotty remove /path/to/your/dotfile

# Sync your dotfiles
dotty sync

# Start watching for changes
dotty watch

# Schedule periodic syncs (every 30 minutes)
dotty schedule --interval 30
```

For more detailed usage instructions, run `dotty --help`.

## ⚙️ Configuration

Dotty uses a TOML configuration file located at `~/.config/dotty/config.toml`. Here's an example configuration:

```toml
[remote]
github_repo = "https://github.com/crazywolf132/dotfiles.git"
github_token = "your_github_token"

[profiles.default]
files = { ".bashrc" = "/home/user/.bashrc", ".vimrc" = "/home/user/.vimrc" }
ignore_patterns = [".git", ".gitignore"]
use_symlinks = false

[profile_detection]
[[profile_detection.rules]]
profile = "work"
conditions = [
    { Hostname = "work-laptop" },
    { EnvVar = { name = "WORK_ENV", value = "true" } }
]

[[profile_detection.rules]]
profile = "personal"
conditions = [{ OS = "macos" }]
```

## 🤝 Contributing

Contributions to Dotty are welcome! Please feel free to submit a Pull Request.

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgements

- Thanks to all the contributors who have helped shape Dotty.
- This project makes use of several fantastic Rust crates, including `clap`, `toml`, `git2`, and more.