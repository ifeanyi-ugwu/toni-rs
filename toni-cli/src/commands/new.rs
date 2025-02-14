use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use colored::*;

#[derive(clap::Args)]
pub struct NewArgs {
    project_name: String,
}

pub async fn execute(args: NewArgs) -> anyhow::Result<()> {
    validate_project_name(&args.project_name)?;
    
    let root_path = PathBuf::from(&args.project_name);
    create_project_structure(&root_path).await?;
    copy_template_files(&root_path).await?;
    post_process_files(&root_path, &args.project_name).await?;

    println!(
        "\n{}",
        format!(
            "✅ Successfully created project '{}'!\n\nNext steps:\n  cd {}\n  cargo build",
            args.project_name, args.project_name
        )
        .green()
    );

    Ok(())
}

fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow!("Project name cannot be empty"));
    }
    
    if name.chars().any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_') {
        return Err(anyhow!("Project name contains invalid characters. Use only alphanumerics, '-' or '_'"));
    }
    
    if PathBuf::from(name).exists() {
        return Err(anyhow!("Directory '{}' already exists", name));
    }

    Ok(())
}

async fn create_project_structure(root: &PathBuf) -> Result<()> {
    let dirs = [
        "src",
        "src/app"
    ];

    for dir in dirs {
        let path = root.join(dir);
        fs::create_dir_all(&path)
            .await
            .with_context(|| format!("Failed to create directory {}", path.display()))?;
    }

    Ok(())
}

async fn copy_template_files(root: &PathBuf) -> Result<()> {
    let cargo_toml = root.join("Cargo.toml");
    let mut file = File::create(&cargo_toml)
        .await
        .context("Failed to create Cargo.toml")?;
    
    file.write_all(include_str!("../templates/new/Cargo.toml").as_bytes())
        .await
        .context("Failed to write Cargo.toml")?;

    write_template_file(
        root,
        "src/main.rs",
        include_str!("../templates/new/src/main.rs"),
    ).await?;

    write_template_file(
        root,
        "src/app/app.module.rs",
        include_str!("../templates/new/src/app/app.module.rs"),
    ).await?;

    write_template_file(
        root,
        "src/app/app.controller.rs",
        include_str!("../templates/new/src/app/app.controller.rs"),
    ).await?;

    write_template_file(
        root,
        "src/app/app.service.rs",
        include_str!("../templates/new/src/app/app.service.rs"),
    ).await?;
    
    write_template_file(
        root,
        "src/app/mod.rs",
        include_str!("../templates/new/src/app/mod.rs"),
    ).await?;

    Ok(())
}

async fn write_template_file(root: &PathBuf, path: &str, content: &str) -> Result<()> {
    let full_path = root.join(path);
    let mut file = File::create(&full_path)
        .await
        .with_context(|| format!("Failed to create {}", full_path.display()))?;
    
    file.write_all(content.as_bytes())
        .await
        .with_context(|| format!("Failed to write to {}", full_path.display()))?;
    
    Ok(())
}

async fn post_process_files(root: &PathBuf, project_name: &str) -> Result<()> {
    let files_to_process = [
        "Cargo.toml",
        "src/main.rs",
        "src/app/app.module.rs",
    ];

    for file in files_to_process {
        let path = root.join(file);
        let content = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read {}", path.display()))?;
        
        let processed = content
            .replace("{{project_name}}", project_name)
            .replace("{{version}}", env!("CARGO_PKG_VERSION"));

        fs::write(&path, processed)
            .await
            .with_context(|| format!("Failed to write processed {}", path.display()))?;
    }

    Ok(())
}