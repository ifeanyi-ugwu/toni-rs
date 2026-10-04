use clap::{Parser, Subcommand};
mod commands;

#[derive(Parser)]
#[command(name = "ulo")]
#[command(version)]
#[command(about = "Ulo Framework CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    New(commands::new::NewArgs),
    Generate(commands::generate::GenerateArgs),
    /// Run the application, rebuilding and restarting on file changes
    #[cfg(feature = "dev")]
    Dev(commands::dev::DevArgs),
    /// The trampoline `ulo dev` starts each child through
    #[cfg(feature = "dev")]
    #[command(name = "__exec", hide = true)]
    Exec(commands::exec::ExecArgs),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        // The trampoline's only work is the exec, so it starts no runtime and no threads.
        #[cfg(feature = "dev")]
        Commands::Exec(args) => commands::exec::execute(args),
        command => tokio::runtime::Runtime::new()?.block_on(run(command)),
    }
}

async fn run(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::New(args) => commands::new::execute(args).await,
        Commands::Generate(args) => commands::generate::execute(args).await,
        #[cfg(feature = "dev")]
        Commands::Dev(args) => commands::dev::execute(args).await,
        #[cfg(feature = "dev")]
        Commands::Exec(args) => commands::exec::execute(args),
    }
}
