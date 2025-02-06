use clap::{Parser, Subcommand};
mod commands;
// use Commands::;

#[derive(Parser)]
#[command(name = "toni")]
#[command(version = "0.0.1")]
#[command(about = "Toni Framework CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    New(commands::new::NewArgs),
    Generate(commands::generate::GenerateArgs),
    // Outros comandos
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::New(args) => commands::new::execute(args).await,
        Commands::Generate(args) => commands::generate::execute(args).await,
    }
}