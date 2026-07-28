use clap::{Parser, Subcommand};
mod commands;

#[derive(Parser)]
#[command(name = "toni")]
#[command(version)]
#[command(about = "Toni Framework CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    New(commands::new::NewArgs),
    Generate(commands::generate::GenerateArgs),
    /// Run the application, rebuilding and restarting on file changes
    Dev(commands::dev::DevArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::New(args) => commands::new::execute(args).await,
        Commands::Generate(args) => commands::generate::execute(args).await,
        Commands::Dev(args) => commands::dev::execute(args).await,
    }
}
