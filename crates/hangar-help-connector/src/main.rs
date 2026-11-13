fn main() -> std::process::ExitCode {
    hangar_help_connector::cli(&std::env::args().skip(1).collect::<Vec<_>>())
}
