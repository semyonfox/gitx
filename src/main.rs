fn main() {
    if let Err(error) = gitx::cli::run() {
        eprintln!("error: {}", gitx::render_error(&error));
        let code = match error {
            gitx::error::GitxError::ToolExit { code, .. } => code,
            _ => 1,
        };
        std::process::exit(code);
    }
}
