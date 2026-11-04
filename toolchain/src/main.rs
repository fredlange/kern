//! `kern`: the command-line front end. `kern host ...` and `kern mcp` are the host and the MCP server; everything
//! else is kern::cli (port of kern/__main__.py).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match argv.first().map(|s| s.as_str()) {
        Some("host") | Some("mcp") => {
            eprintln!("kern {}: not ported to the Rust toolchain yet (use `python -m kern.{}`)", argv[0], argv[0]);
            2
        }
        Some("oracle") => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            kern::oracle::main(&argv[1..], &mut out, &mut err)
        }
        _ => kern::cli::main(&argv),
    };
    std::process::exit(code);
}
