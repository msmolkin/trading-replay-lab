use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = env::args();
    let program = arguments.next().unwrap_or_else(|| "sim-cli".into());
    let Some(command) = arguments.next() else {
        eprintln!("usage: {program} <verify|ledger> <proof-bundle.json>");
        return ExitCode::from(2);
    };
    let Some(path) = arguments.next() else {
        eprintln!("usage: {program} <verify|ledger> <proof-bundle.json>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: {program} <verify|ledger> <proof-bundle.json>");
        return ExitCode::from(2);
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            return ExitCode::from(2);
        }
    };
    match command.as_str() {
        "verify" => {
            let report = sim_cli::verify_bytes(&bytes);
            println!("{}", report.to_json());
            if report.valid {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        "ledger" => match sim_cli::inspect_ledger_bytes(&bytes) {
            Ok(inspection) => {
                println!("{}", inspection.to_json());
                ExitCode::SUCCESS
            }
            Err(failure) => {
                println!(
                    "{{\"valid\":false,\"failure_code\":\"{}\",\"index\":{},\"detail\":\"{}\"}}",
                    failure.code.code(),
                    failure
                        .index
                        .map_or_else(|| "null".to_owned(), |index| index.to_string()),
                    escape_json(&failure.detail)
                );
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("unknown command {command:?}; expected verify or ledger");
            ExitCode::from(2)
        }
    }
}

fn escape_json(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => output.push('?'),
            character => output.push(character),
        }
    }
    output
}
