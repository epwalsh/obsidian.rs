use std::path::PathBuf;

use clap::Parser;
use obsidian_core::{Vault, VaultError};

#[derive(Debug, Parser)]
#[command(
    name = "obsidian-lsp",
    about = "Language Server Protocol server for an Obsidian vault"
)]
pub struct Args {
    /// Path to the Obsidian vault. Overrides the OBSIDIAN_VAULT environment variable.
    #[arg(long)]
    pub vault: Option<PathBuf>,
}

pub fn resolve_vault_path(cli_vault: Option<PathBuf>, env_vault: Option<PathBuf>) -> Result<PathBuf, VaultError> {
    if let Some(path) = cli_vault {
        return Ok(path);
    }
    if let Some(path) = env_vault {
        return Ok(path);
    }

    Ok(Vault::open_from_cwd()?.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::{LazyLock, Mutex};

    static CWD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct CurrentDirGuard(PathBuf);

    impl CurrentDirGuard {
        fn change_to(path: &Path) -> Self {
            let original = std::env::current_dir().unwrap();
            std::env::set_current_dir(path).unwrap();
            Self(original)
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }

    #[test]
    fn resolve_vault_path_prefers_cli_arg() {
        let cli_vault = PathBuf::from("/tmp/cli-vault");
        let env_vault = PathBuf::from("/tmp/env-vault");

        let resolved = resolve_vault_path(Some(cli_vault.clone()), Some(env_vault)).unwrap();

        assert_eq!(resolved, cli_vault);
    }

    #[test]
    fn resolve_vault_path_falls_back_to_env_var() {
        let env_vault = PathBuf::from("/tmp/env-vault");

        let resolved = resolve_vault_path(None, Some(env_vault.clone())).unwrap();

        assert_eq!(resolved, env_vault);
    }

    #[test]
    fn resolve_vault_path_falls_back_to_nearest_obsidian_dir() {
        let _cwd_guard = CWD_LOCK.lock().unwrap();
        let vault_dir = tempfile::tempdir().unwrap();
        let nested_dir = vault_dir.path().join("notes/daily");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();
        let _current_dir = CurrentDirGuard::change_to(&nested_dir);

        let resolved = resolve_vault_path(None, None).unwrap();

        assert_eq!(
            resolved.canonicalize().unwrap(),
            vault_dir.path().canonicalize().unwrap()
        );
    }
}
