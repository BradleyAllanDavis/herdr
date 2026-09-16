use crate::api::schema::{
    EmptyParams, Method, Request, SoundVolumeSetParams, SoundVolumeStepDirection,
    SoundVolumeStepParams,
};

pub(super) fn run_sound_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_sound_help();
        return Ok(2);
    };

    match subcommand {
        "volume" => run_sound_volume_command(&args[1..]),
        "help" | "--help" | "-h" => {
            print_sound_help();
            Ok(0)
        }
        _ => {
            print_sound_help();
            Ok(2)
        }
    }
}

fn run_sound_volume_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(|arg| arg.as_str()) {
        None | Some("--json") => sound_volume_get(args),
        Some("set") => sound_volume_set(&args[1..]),
        Some("up") => sound_volume_step(SoundVolumeStepDirection::Up, &args[1..]),
        Some("down") => sound_volume_step(SoundVolumeStepDirection::Down, &args[1..]),
        Some("help" | "--help" | "-h") => {
            print_sound_volume_help();
            Ok(0)
        }
        Some(other) => {
            eprintln!("unknown sound volume subcommand: {other}");
            print_sound_volume_help();
            Ok(2)
        }
    }
}

fn sound_volume_get(args: &[String]) -> std::io::Result<i32> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => {
            eprintln!("usage: herdr sound volume [--json]");
            return Ok(2);
        }
    };

    let response = super::send_request(&Request {
        id: "cli:sound:volume:get".into(),
        method: Method::SoundVolumeGet(EmptyParams::default()),
    })?;
    if json || response.get("error").is_some() {
        return super::print_response(&response);
    }

    print_sound_volume_status(&response);
    Ok(0)
}

fn sound_volume_set(args: &[String]) -> std::io::Result<i32> {
    let volume = match parse_sound_volume_set_args(args) {
        Ok(volume) => volume,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    let response = super::send_request(&Request {
        id: "cli:sound:volume:set".into(),
        method: Method::SoundVolumeSet(SoundVolumeSetParams { volume }),
    })?;
    if response.get("error").is_some() {
        return super::print_response(&response);
    }

    print_sound_volume_status(&response);
    Ok(0)
}

fn sound_volume_step(direction: SoundVolumeStepDirection, args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        let word = match direction {
            SoundVolumeStepDirection::Up => "up",
            SoundVolumeStepDirection::Down => "down",
        };
        eprintln!("usage: herdr sound volume {word}");
        return Ok(2);
    }

    let response = super::send_request(&Request {
        id: "cli:sound:volume:step".into(),
        method: Method::SoundVolumeStep(SoundVolumeStepParams { direction }),
    })?;
    if response.get("error").is_some() {
        return super::print_response(&response);
    }

    print_sound_volume_status(&response);
    Ok(0)
}

fn parse_sound_volume_set_args(args: &[String]) -> Result<f32, String> {
    let [raw] = args else {
        return Err("usage: herdr sound volume set <0.0-1.0>".to_string());
    };
    raw.parse::<f32>()
        .map_err(|_| format!("invalid volume: {raw} (expected a number between 0.0 and 1.0)"))
}

fn print_sound_volume_status(response: &serde_json::Value) {
    println!("{}", format_sound_volume_status(response));
}

fn format_sound_volume_status(response: &serde_json::Value) -> String {
    let volume = response["result"]["volume"].as_f64().unwrap_or(0.0);
    let enabled = response["result"]["enabled"].as_bool().unwrap_or(true);
    let percent = (volume * 100.0).round() as i64;
    if enabled {
        format!("volume: {percent}%")
    } else {
        format!("volume: {percent}% (muted — ui.sound.enabled = false)")
    }
}

fn print_sound_help() {
    eprintln!("herdr sound commands:");
    eprintln!("  herdr sound volume                print the live server volume");
    eprintln!("  herdr sound volume set <0.0-1.0>   set the live server volume");
    eprintln!("  herdr sound volume up              raise the live server volume by one step");
    eprintln!("  herdr sound volume down            lower the live server volume by one step");
}

fn print_sound_volume_help() {
    eprintln!("usage:");
    eprintln!("  herdr sound volume [--json]");
    eprintln!("  herdr sound volume set <0.0-1.0>");
    eprintln!("  herdr sound volume up");
    eprintln!("  herdr sound volume down");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn set_args_parse_a_valid_volume() {
        assert_eq!(parse_sound_volume_set_args(&args(&["0.35"])), Ok(0.35));
    }

    #[test]
    fn set_args_reject_missing_value() {
        assert_eq!(
            parse_sound_volume_set_args(&args(&[])),
            Err("usage: herdr sound volume set <0.0-1.0>".to_string())
        );
    }

    #[test]
    fn set_args_reject_extra_arguments() {
        assert_eq!(
            parse_sound_volume_set_args(&args(&["0.5", "extra"])),
            Err("usage: herdr sound volume set <0.0-1.0>".to_string())
        );
    }

    #[test]
    fn set_args_reject_non_numeric_value() {
        assert_eq!(
            parse_sound_volume_set_args(&args(&["loud"])),
            Err("invalid volume: loud (expected a number between 0.0 and 1.0)".to_string())
        );
    }

    #[test]
    fn set_args_accept_out_of_range_values_and_let_the_server_clamp() {
        // The CLI parses any f32; clamping to 0.0..=1.0 is the server's job
        // (Method::SoundVolumeSet), same division of responsibility as every
        // other config-adjacent CLI verb in this codebase.
        assert_eq!(parse_sound_volume_set_args(&args(&["1.5"])), Ok(1.5));
    }

    #[test]
    fn status_reports_percent_when_enabled() {
        let response = serde_json::json!({"result": {"volume": 0.45, "enabled": true}});
        assert_eq!(format_sound_volume_status(&response), "volume: 45%");
    }

    #[test]
    fn status_reports_muted_when_disabled_regardless_of_volume() {
        let response = serde_json::json!({"result": {"volume": 0.8, "enabled": false}});
        assert_eq!(
            format_sound_volume_status(&response),
            "volume: 80% (muted — ui.sound.enabled = false)"
        );
    }

    #[test]
    fn status_rounds_and_handles_extremes() {
        let zero = serde_json::json!({"result": {"volume": 0.0, "enabled": true}});
        let full = serde_json::json!({"result": {"volume": 1.0, "enabled": true}});
        assert_eq!(format_sound_volume_status(&zero), "volume: 0%");
        assert_eq!(format_sound_volume_status(&full), "volume: 100%");
    }
}
