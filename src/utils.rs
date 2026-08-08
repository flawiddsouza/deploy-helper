use colored::Colorize;
use indexmap::IndexMap;
use minijinja::{value::Value as MiniJinjaValue, Environment, UndefinedBehavior};
use serde::Deserialize;
use serde_json::Value;
use simple_expand_tilde::expand_tilde;
use ssh2::Session;
use std::fs;
use std::io::{self, prelude::*};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

pub(crate) fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// Renders an `environment:` map to final values. Values go through placeholder
// substitution; keys must be plain identifiers so nothing can smuggle shell
// syntax through a key when the map is turned into export lines.
pub fn render_env_values(
    env: &IndexMap<String, String>,
    vars_map: &IndexMap<String, Value>,
) -> Result<IndexMap<String, String>, String> {
    let mut rendered = IndexMap::new();
    for (key, value) in env {
        let key_ok = !key.is_empty()
            && !key.chars().next().unwrap().is_ascii_digit()
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !key_ok {
            return Err(format!(
                "invalid environment key '{}': keys must be identifiers like B2_ACCOUNT_ID",
                key
            ));
        }
        rendered.insert(key.clone(), replace_placeholders(value, vars_map));
    }
    Ok(rendered)
}

// The same map as single-quote escaped `export KEY='value'` lines, for paths
// where the environment has to travel inside the command text (shell blocks,
// become wrappers, remote exec).
pub fn env_export_lines(rendered: &IndexMap<String, String>) -> Vec<String> {
    rendered
        .iter()
        .map(|(k, v)| format!("export {}={}", k, shell_escape(v)))
        .collect()
}

// Permission modes are restricted to plain octal so they can be spliced into
// shell commands without quoting concerns.
pub fn validate_mode(mode: &str) -> Result<(), String> {
    let ok = !mode.is_empty() && mode.len() <= 4 && mode.chars().all(|c| ('0'..='7').contains(&c));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid mode '{}': expected 1 to 4 octal digits, like \"0600\" or \"755\"",
            mode
        ))
    }
}

// The two staged-write command builders below place a file at `dest` with
// `mode` applied before it becomes visible there: the temp copy sits next to
// dest, is created under umask 077 (so it never exceeds 0600), gets chmod'ed
// to the requested mode, and an atomic mv replaces dest. The file is never
// readable beyond `mode` at any point.
fn mode_dest_tmp(dest: &str) -> String {
    format!("{}.deploy-helper-tmp", dest)
}

// stdin is piped into the temp file (`cat > tmp`).
fn write_pipe_command(dest: &str, mode: Option<&str>) -> String {
    match mode {
        None => format!("cat > {}", shell_escape(dest)),
        Some(m) => {
            let dtmp = mode_dest_tmp(dest);
            format!(
                "umask 077 && cat > {dtmp} && chmod {m} {dtmp} && mv -f {dtmp} {dst}",
                dtmp = shell_escape(&dtmp),
                m = m,
                dst = shell_escape(dest)
            )
        }
    }
}

// An already-staged file at `src_tmp` is copied into place.
fn place_file_command(src_tmp: &str, dest: &str, mode: Option<&str>) -> String {
    match mode {
        None => format!("cp {} {}", shell_escape(src_tmp), shell_escape(dest)),
        Some(m) => {
            let dtmp = mode_dest_tmp(dest);
            format!(
                "umask 077 && cp {src} {dtmp} && chmod {m} {dtmp} && mv -f {dtmp} {dst}",
                src = shell_escape(src_tmp),
                dtmp = shell_escape(&dtmp),
                m = m,
                dst = shell_escape(dest)
            )
        }
    }
}

pub fn wrap_become_command(command: &str, method: &str, password: Option<&str>) -> String {
    if method == "su" {
        if let Some(pw) = password {
            format!(
                "printf '%s\\n' {} | su -c {}",
                shell_escape(pw),
                shell_escape(command)
            )
        } else {
            format!("su -c {}", shell_escape(command))
        }
    } else if let Some(pw) = password {
        format!(
            "printf '%s\\n' {} | {} -S -p '' sh -c {}",
            shell_escape(pw),
            method,
            shell_escape(command)
        )
    } else {
        format!("{} sh -c {}", method, shell_escape(command))
    }
}

pub fn replace_placeholders(msg: &str, vars: &IndexMap<String, Value>) -> String {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_filter("from_json", from_json_filter);
    env.add_filter("from_env", from_env_filter);
    let template = env.template_from_str(msg).unwrap();
    let mut context = IndexMap::new();

    for (key, value) in vars {
        context.insert(key.clone(), value.clone());
    }

    // Debug print to verify context
    // println!("Context: {:?}", context);

    template.render(&context).unwrap_or_else(|err| {
        if let minijinja::ErrorKind::UndefinedError = err.kind() {
            eprintln!(
                "{}",
                format!(
                    "One or more of the variables are undefined in:\n\"{}\"",
                    msg
                )
                .red()
            );
            eprintln!("{}", format!("Available vars: {:#?}", context).red());
        } else {
            eprintln!("{}", format!("Error rendering template: {}", err).red());
        }

        exit(1);
    })
}

pub fn replace_placeholders_vars(msg: &str, vars: &IndexMap<String, Value>) -> Value {
    let rendered_str = replace_placeholders(msg, vars);

    if uses_template_filter(msg, "from_json") {
        serde_json::from_str(&rendered_str).unwrap_or_else(|err| {
            eprintln!(
                "{}",
                format!("Error parsing JSON: {}:\n{}\nat {}", err, rendered_str, msg).red()
            );
            exit(1);
        })
    } else if uses_template_filter(msg, "from_env") {
        parse_env_output(&rendered_str).unwrap_or_else(|err| {
            eprintln!(
                "{}",
                format!("Error parsing env: {}\nat {}", err, msg).red()
            );
            exit(1);
        })
    } else {
        Value::String(rendered_str)
    }
}

fn uses_template_filter(template: &str, filter: &str) -> bool {
    let template = template.trim();
    let Some(expression) = template
        .strip_prefix("{{")
        .and_then(|value| value.strip_suffix("}}"))
    else {
        return false;
    };
    if expression.contains("{{") || expression.contains("}}") {
        return false;
    }

    let mut quote = None;
    let mut escaped = false;
    for (index, character) in expression.char_indices() {
        if let Some(quote_character) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote_character {
                quote = None;
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            continue;
        }
        if character != '|' {
            continue;
        }
        let remainder = expression[index + character.len_utf8()..].trim_start();
        if remainder.strip_prefix(filter).is_some_and(|after_filter| {
            after_filter
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_')
        }) {
            return true;
        }
    }
    false
}

fn heredoc_delimiter(line: &str) -> Option<String> {
    let pos = line.find("<<")?;
    let after = line[pos + 2..].trim_start();
    let after = after.strip_prefix('-').unwrap_or(after).trim_start();
    let raw = if let Some(rest) = after.strip_prefix('\'') {
        rest.split('\'').next()?
    } else if let Some(rest) = after.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        after.split(|c: char| c.is_whitespace()).next()?
    };
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn token_depth_delta(word: &str, cmd_position: bool) -> i32 {
    if !cmd_position {
        return 0;
    }
    match word {
        "if" | "case" | "for" | "while" | "until" | "select" => 1,
        "fi" | "esac" | "done" => -1,
        _ => 0,
    }
}

// Scans one logical line and updates a running nesting depth for shell
// compound commands (if/fi, case/esac, for|while|until|select/done). Used
// so multi-line compound blocks stay together as a single command instead
// of each line being dispatched separately.
fn update_depth(line: &str, depth: &mut i32) {
    let mut chars = line.chars().peekable();
    let mut cmd_position = true;
    let mut word = String::new();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            word.push(c);
            continue;
        }
        if in_double {
            if c == '\\' {
                if let Some(&next) = chars.peek() {
                    word.push('\\');
                    word.push(next);
                    chars.next();
                    continue;
                }
            }
            if c == '"' {
                in_double = false;
            }
            word.push(c);
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                word.push(c);
            }
            '"' => {
                in_double = true;
                word.push(c);
            }
            '\\' => {
                if let Some(&next) = chars.peek() {
                    word.push(next);
                    chars.next();
                }
            }
            '#' if word.is_empty() => break,
            ' ' | '\t' => {
                if !word.is_empty() {
                    *depth += token_depth_delta(&word, cmd_position);
                    if *depth < 0 {
                        *depth = 0;
                    }
                    cmd_position = false;
                    word.clear();
                }
            }
            ';' | '&' | '|' => {
                if !word.is_empty() {
                    *depth += token_depth_delta(&word, cmd_position);
                    if *depth < 0 {
                        *depth = 0;
                    }
                    word.clear();
                }
                cmd_position = true;
            }
            _ => word.push(c),
        }
    }
    if !word.is_empty() {
        *depth += token_depth_delta(&word, cmd_position);
        if *depth < 0 {
            *depth = 0;
        }
    }
}

pub fn split_commands(input: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut current_command = String::new();
    let mut heredoc_end: Option<String> = None;
    let mut depth: i32 = 0;
    let mut pending_continuation = String::new();

    for line in input.lines() {
        if let Some(ref delimiter) = heredoc_end {
            current_command.push('\n');
            current_command.push_str(line);
            if line.trim() == delimiter.as_str() {
                heredoc_end = None;
                if depth == 0 {
                    commands.push(std::mem::take(&mut current_command));
                }
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.ends_with('\\') {
            let clean_line = trimmed.trim_end_matches('\\').trim_end();
            if !pending_continuation.is_empty() {
                pending_continuation.push(' ');
            }
            pending_continuation.push_str(clean_line);
            continue;
        }

        let logical_line = if pending_continuation.is_empty() {
            trimmed.to_string()
        } else {
            let mut s = std::mem::take(&mut pending_continuation);
            s.push(' ');
            s.push_str(trimmed);
            s
        };

        if !current_command.is_empty() {
            current_command.push('\n');
        }
        current_command.push_str(&logical_line);

        if let Some(delim) = heredoc_delimiter(&logical_line) {
            heredoc_end = Some(delim);
            continue;
        }

        update_depth(&logical_line, &mut depth);

        if depth == 0 {
            commands.push(std::mem::take(&mut current_command));
        }
    }

    if !current_command.is_empty() {
        commands.push(current_command);
    }

    commands
}

pub fn from_json_filter(value: MiniJinjaValue) -> MiniJinjaValue {
    value
}

pub fn from_env_filter(value: MiniJinjaValue) -> MiniJinjaValue {
    value
}

fn parse_env_output(input: &str) -> Result<Value, String> {
    let mut parsed = serde_json::Map::new();
    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {} must be KEY=VALUE", line_number));
        };
        let mut chars = key.chars();
        let key_valid = chars
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && chars.all(|character| character == '_' || character.is_ascii_alphanumeric());
        if !key_valid {
            return Err(format!("line {} has invalid key '{}'", line_number, key));
        }
        if parsed
            .insert(key.to_string(), Value::String(value.to_string()))
            .is_some()
        {
            return Err(format!("line {} repeats key '{}'", line_number, key));
        }
    }
    Ok(Value::Object(parsed))
}

fn annotate_yaml_error(filename: &str, contents: &str, err: serde_yaml::Error) -> String {
    let msg = err.to_string();
    if !msg.contains("invalid type: map, expected a string") {
        return format!("{}: {}", filename, msg);
    }
    let Some(loc) = err.location() else {
        return format!("{}: {}", filename, msg);
    };
    let line_no = loc.line();
    let Some(line) = contents.lines().nth(line_no.saturating_sub(1)) else {
        return format!("{}: {}", filename, msg);
    };
    if !line.contains("{{") {
        return format!("{}: {}", filename, msg);
    }
    format!(
        "{}: line {} has an unquoted {{{{ ... }}}} value:\n    {}\n  YAML reads a leading {{ as the start of an inline object, so {{{{ var }}}} gets parsed as a nested object instead of text.\n  Wrap it in quotes so YAML treats it as a string, e.g. \"{{{{ var }}}}\" or \"{{{{ var }}}}/path\".",
        filename,
        line_no,
        line.trim_end(),
    )
}

fn read_file_or_exit(filename: &str) -> String {
    fs::read_to_string(filename).unwrap_or_else(|e| {
        let msg = if e.kind() == io::ErrorKind::NotFound {
            let location = if Path::new(filename).parent() == Some(Path::new("")) {
                " in current directory"
            } else {
                " at given path"
            };
            format!("{}: not found{}", filename, location)
        } else {
            format!("Failed to read {}: {}", filename, e)
        };
        eprintln!("{}", msg.red());
        exit(1);
    })
}

pub fn resolve_src_path(deploy_file_dir: &Path, src: &str) -> PathBuf {
    let p = Path::new(src);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        deploy_file_dir.join(p)
    }
}

pub fn read_yaml<T>(filename: &str) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let contents = read_file_or_exit(filename);
    serde_yaml::from_str(&contents).unwrap_or_else(|e| {
        eprintln!("{}", annotate_yaml_error(filename, &contents, e).red());
        exit(1);
    })
}

pub fn read_yaml_multi<T>(filename: &str) -> Vec<T>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = read_file_or_exit(filename);
    let mut results = Vec::new();

    for document in serde_yaml::Deserializer::from_str(&contents) {
        let item = T::deserialize(document).unwrap_or_else(|e| {
            eprintln!("{}", annotate_yaml_error(filename, &contents, e).red());
            exit(1);
        });
        results.push(item);
    }

    results
}

pub fn setup_ssh_session(
    host: &str,
    port: u16,
    user: &str,
    password: Option<&str>,
    ssh_key_path: Option<&str>,
) -> Result<Session, Box<dyn std::error::Error>> {
    let tcp = TcpStream::connect((host, port))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    if let Some(key_path) = ssh_key_path {
        let resolved_key_path = expand_tilde(key_path).ok_or("Failed to resolve home directory")?;
        session.userauth_pubkey_file(user, None, &resolved_key_path, None)?;
    } else if let Some(pwd) = password {
        session.userauth_password(user, pwd)?;
    } else {
        return Err("Either ssh_key_path or password must be provided".into());
    }

    if !session.authenticated() {
        return Err("Authentication failed".into());
    }

    Ok(session)
}

pub fn execute_ssh_command(
    session: &Session,
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
    login_shell: bool,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    session.set_blocking(true);
    let mut channel = session.channel_session()?;

    // Use $SHELL -l -i so login files (.zprofile/.bash_profile) and interactive
    // files (.zshrc/.bashrc) are both sourced — required for PATH entries added
    // by tools like bun/nvm that only appear in .bashrc/.zshrc.
    let final_cmd = if login_shell {
        let base = if let Some(dir) = chdir {
            format!("cd {} && {}", dir, command)
        } else {
            command.to_string()
        };
        let sh_arg = format!("exec \"$SHELL\" -l -i -c {}", shell_escape(&base));
        format!("sh -c {}", shell_escape(&sh_arg))
    } else if let Some(dir) = chdir {
        let base = format!("cd {} && {}", dir, command);
        if use_shell {
            format!("sh -c {}", shell_escape(&base))
        } else {
            base
        }
    } else if use_shell {
        format!("sh -c {}", shell_escape(command))
    } else {
        command.to_string()
    };

    channel.exec(&final_cmd)?;

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut stdout_buffer = [0; 1024];
    let mut stderr_buffer = [0; 1024];

    loop {
        match channel.read(&mut stdout_buffer) {
            Ok(read_bytes) => {
                if read_bytes > 0 {
                    let output = String::from_utf8_lossy(&stdout_buffer[..read_bytes]);
                    stdout.push_str(&output);
                    if display_output {
                        print!("{}", output.white());
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.into()),
        }

        match channel.stderr().read(&mut stderr_buffer) {
            Ok(read_bytes) => {
                if read_bytes > 0 {
                    let error_output = String::from_utf8_lossy(&stderr_buffer[..read_bytes]);
                    stderr.push_str(&error_output);
                    if display_output {
                        print!("{}", error_output.red());
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.into()),
        }

        if channel.eof() {
            break;
        }
    }

    channel.wait_close()?;
    let exit_status = channel.exit_status()?;

    // BufReader::lines() used in local execution strips trailing newlines;
    // match that behaviour here so registered output is consistent.
    let stdout = stdout.trim_end_matches(['\n', '\r']).to_string();
    let stderr = stderr.trim_end_matches(['\n', '\r']).to_string();

    Ok((stdout, stderr, exit_status))
}

pub fn execute_local_command(
    command: &str,
    use_shell: bool,
    display_output: bool,
    chdir: Option<&str>,
    login_shell: bool,
    env: Option<&IndexMap<String, String>>,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    let mut cmd = if login_shell && !cfg!(windows) {
        let sh_arg = format!("exec \"$SHELL\" -l -i -c {}", shell_escape(command));
        let mut c = Command::new("sh");
        c.arg("-c").arg(sh_arg);
        c
    } else if use_shell {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    } else {
        let parts =
            shell_words::split(command).map_err(|e| format!("Failed to parse command: {}", e))?;
        let mut cmd = Command::new(&parts[0]);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }
        cmd
    };

    if let Some(dir) = chdir {
        cmd.current_dir(dir);
    }

    if let Some(env) = env {
        cmd.envs(env);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or("Failed to open stdout")?;
    let stderr = child.stderr.take().ok_or("Failed to open stderr")?;

    let mut stdout_str = String::new();
    let mut stderr_str = String::new();

    let stdout_reader = std::io::BufReader::new(stdout).lines();
    let stderr_reader = std::io::BufReader::new(stderr).lines();

    for line in stdout_reader {
        if let Ok(line) = line {
            if display_output {
                println!("{}", line.white());
            }
            if !stdout_str.is_empty() {
                stdout_str.push('\n');
            }
            stdout_str.push_str(&line);
        }
    }

    for line in stderr_reader {
        if let Ok(line) = line {
            if display_output {
                eprintln!("{}", line.red());
            }
            if !stderr_str.is_empty() {
                stderr_str.push('\n');
            }
            stderr_str.push_str(&line);
        }
    }

    let exit_status = child.wait()?.code().unwrap_or(-1);

    Ok((stdout_str, stderr_str, exit_status))
}

// Returns true if `path` exists on the target (localhost or the remote session),
// checked with `test -e`. Backs the `creates:`/`removes:` task guards.
pub fn path_exists_on_target(
    path: &str,
    is_localhost: bool,
    session: Option<&Session>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let cmd = format!("test -e {}", shell_escape(path));
    let (_stdout, _stderr, exit_status) = if is_localhost {
        execute_local_command(&cmd, true, false, None, false, None)?
    } else {
        let session = session.ok_or("path_exists_on_target: remote target requires session")?;
        execute_ssh_command(session, &cmd, true, false, None, false)?
    };
    Ok(exit_status == 0)
}

pub fn execute_ssh_doas_with_pty(
    session: &Session,
    command: &str,
    password: &str,
    display_output: bool,
    chdir: Option<&str>,
    login_shell: bool,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    session.set_blocking(true);
    let mut channel = session.channel_session()?;
    channel.request_pty("xterm", None, None)?;

    let final_cmd = if login_shell {
        let base = if let Some(dir) = chdir {
            format!("cd {} && {}", shell_escape(dir), command)
        } else {
            command.to_string()
        };
        let sh_arg = format!("exec \"$SHELL\" -l -i -c {}", shell_escape(&base));
        format!("sh -c {}", shell_escape(&sh_arg))
    } else if let Some(dir) = chdir {
        format!("cd {} && {}", shell_escape(dir), command)
    } else {
        command.to_string()
    };

    channel.exec(&final_cmd)?;

    let mut stdout_buf = [0u8; 1024];
    let mut stdout = String::new();
    // doas flushes any input typed before it prints its prompt, so the password
    // must be sent only after the prompt appears -- writing it up front gets
    // discarded and doas blocks forever waiting for input.
    let mut password_sent = false;

    loop {
        match channel.read(&mut stdout_buf) {
            Ok(n) if n > 0 => {
                let output = String::from_utf8_lossy(&stdout_buf[..n]);
                stdout.push_str(&output);
                if display_output {
                    print!("{}", output.white());
                }
                if !password_sent && stdout.to_lowercase().contains("password") {
                    channel.write_all(format!("{}\n", password).as_bytes())?;
                    channel.flush()?;
                    password_sent = true;
                }
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        if channel.eof() {
            break;
        }
    }

    channel.wait_close()?;
    let exit_code = channel.exit_status()?;

    let stdout = stdout.trim_end_matches(['\n', '\r']).to_string();
    Ok((stdout, String::new(), exit_code))
}

#[cfg(unix)]
pub fn execute_local_doas_with_pty(
    command: &str,
    password: &str,
    display_output: bool,
    chdir: Option<&str>,
    login_shell: bool,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    use expectrl::{
        process::{unix::WaitStatus, Healthcheck},
        Eof, Expect,
    };

    let mut cmd = std::process::Command::new("sh");
    if login_shell {
        cmd.arg("-c")
            .arg(format!("exec \"$SHELL\" -l -i -c {}", shell_escape(command)));
    } else {
        cmd.arg("-c").arg(command);
    }
    if let Some(dir) = chdir {
        cmd.current_dir(dir);
    }

    let mut session = expectrl::Session::spawn(cmd)?;
    // doas discards input typed before its prompt appears, so wait for the
    // prompt before sending the password instead of writing it up front.
    let prompt = session.expect("password")?;
    session.send_line(password)?;

    // `expect` consumes the bytes up to and including the match, so the Eof
    // capture alone would drop the doas prompt. Join the prompt bytes with
    // everything read afterwards so the captured stream matches
    // execute_ssh_doas_with_pty, which keeps the whole PTY stream.
    let rest = session.expect(Eof)?;
    let mut combined_bytes = prompt.as_bytes().to_vec();
    combined_bytes.extend_from_slice(rest.as_bytes());
    let combined = String::from_utf8_lossy(&combined_bytes).into_owned();

    if display_output {
        print!("{}", combined.white());
    }

    let exit_code = loop {
        match session.get_status() {
            Ok(WaitStatus::Exited(_, code)) => break code,
            Ok(WaitStatus::Signaled(_, _, _)) => break 1,
            // Other states (still running, stopped, continued) -- keep polling.
            Ok(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            // A status error (e.g. the child was already reaped) would otherwise
            // loop forever, so surface it as a task failure instead.
            Err(e) => return Err(e.into()),
        }
    };

    Ok((combined.trim_end_matches(['\n', '\r']).to_string(), String::new(), exit_code))
}

// A doas PTY merges its authentication prompt into stdout. Remove only that
// protocol text before an action inspects or registers the command output.
// The command's own leading whitespace must remain intact for exact matching.
fn strip_doas_password_prompt(output: &str) -> String {
    let output_lower = output.to_ascii_lowercase();
    let Some(prompt_start) = output_lower.find("password:") else {
        return output.to_string();
    };

    let mut command_output = &output[prompt_start + "password:".len()..];
    if let Some(rest) = command_output.strip_prefix(' ') {
        command_output = rest;
    }
    if let Some(rest) = command_output.strip_prefix("\r\n") {
        command_output = rest;
    } else if let Some(rest) = command_output.strip_prefix(['\r', '\n']) {
        command_output = rest;
    }
    command_output.to_string()
}

// Runs `command` on the target through the same become/doas plumbing the file
// writes use, without displaying output. Returns (stdout, stderr, exit_code);
// the doas-PTY path merges stderr into stdout.
pub fn run_shell_on_target(
    command: &str,
    is_localhost: bool,
    session: Option<&Session>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    run_shell_on_target_with_context(
        command,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
        None,
        false,
        None,
    )
}

// Runs one shell command with the task execution context and returns its raw
// result without displaying output. Environment exports stay inside a become
// wrapper so privilege escalation cannot discard them.
#[allow(clippy::too_many_arguments)]
pub fn run_shell_on_target_with_context(
    command: &str,
    is_localhost: bool,
    session: Option<&Session>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    chdir: Option<&str>,
    login_shell: bool,
    env: Option<&IndexMap<String, String>>,
) -> Result<(String, String, i32), Box<dyn std::error::Error>> {
    let command_with_env = match env {
        Some(env) if !env.is_empty() => format!(
            "{{\n{}\n{}\n}}",
            env_export_lines(env).join("\n"),
            command
        ),
        _ => command.to_string(),
    };

    // doas reads its password from /dev/tty, so a doas-with-password command
    // must go through a PTY, not the piped wrap_become path.
    let doas_pw = if become_enabled && become_method == "doas" {
        become_password.filter(|s| !s.is_empty())
    } else {
        None
    };

    if let Some(password) = doas_pw {
        let doas_cmd = wrap_become_command(&command_with_env, "doas", None);
        let result = if is_localhost {
            #[cfg(unix)]
            {
                execute_local_doas_with_pty(&doas_cmd, password, false, chdir, login_shell)
            }
            #[cfg(not(unix))]
            {
                let _ = password;
                Err(
                    "doas with become_password is not supported on non-Unix platforms".into(),
                )
            }
        } else {
            let session =
                session.ok_or("run_shell_on_target_with_context: remote target requires session")?;
            execute_ssh_doas_with_pty(session, &doas_cmd, password, false, chdir, login_shell)
        };
        let (stdout, stderr, rc) = result?;
        return Ok((strip_doas_password_prompt(&stdout), stderr, rc));
    }

    let cmd = if become_enabled {
        wrap_become_command(&command_with_env, become_method, become_password)
    } else {
        command_with_env
    };
    if is_localhost {
        execute_local_command(&cmd, true, false, chdir, login_shell, None)
    } else {
        let session =
            session.ok_or("run_shell_on_target_with_context: remote target requires session")?;
        execute_ssh_command(session, &cmd, true, false, chdir, login_shell)
    }
}

// Writes `bytes` to a privileged `dest` on localhost via doas+password. doas
// needs a tty, so the bytes are staged to a user-owned temp file first and then
// copied into place under a PTY-authenticated doas (mirrors the remote path).
#[cfg(unix)]
fn write_local_doas(
    bytes: &[u8],
    dest: &str,
    password: &str,
    mode: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = format!("/tmp/deploy-helper-{}-{}", nanos, std::process::id());
    std::fs::write(&tmp_path, bytes)
        .map_err(|e| format!("Failed to write {}: {}", tmp_path, e))?;
    if mode.is_some() {
        // The staged copy must never be readable beyond the requested mode.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to chmod {}: {}", tmp_path, e))?;
    }

    let inner = place_file_command(&tmp_path, dest, mode);
    let doas_cmd = wrap_become_command(&inner, "doas", None);
    let result = execute_local_doas_with_pty(&doas_cmd, password, false, None, false);
    let _ = std::fs::remove_file(&tmp_path);

    let (out, _stderr, code) = result?;
    if code != 0 {
        return Err(format!("Failed to write {}: exit {}: {}", dest, code, out.trim()).into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_local_doas(
    _bytes: &[u8],
    _dest: &str,
    _password: &str,
    _mode: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("doas with become_password is not supported on non-Unix platforms".into())
}

pub fn write_to_target(
    bytes: &[u8],
    dest: &str,
    is_localhost: bool,
    session: Option<&Session>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    mode: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // doas can't take a piped password (needs a tty), so it routes through a
    // PTY helper instead of the standard wrap_become_command path below.
    let doas_pw = if become_enabled && become_method == "doas" {
        become_password.filter(|s| !s.is_empty())
    } else {
        None
    };

    if is_localhost {
        if become_enabled {
            if let Some(password) = doas_pw {
                return write_local_doas(bytes, dest, password, mode);
            }
            let inner = write_pipe_command(dest, mode);
            let wrapped = wrap_become_command(&inner, become_method, become_password);
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(&wrapped);
            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            let mut child = cmd
                .spawn()
                .map_err(|e| format!("Failed to spawn write process: {}", e))?;
            {
                let stdin = child
                    .stdin
                    .as_mut()
                    .ok_or("Failed to open stdin for write process")?;
                stdin
                    .write_all(bytes)
                    .map_err(|e| format!("Failed to write to {}: {}", dest, e))?;
            }
            let output = child
                .wait_with_output()
                .map_err(|e| format!("Failed to wait for write process: {}", e))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "Failed to write {}: exit {}: {}",
                    dest,
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                )
                .into());
            }
            return Ok(());
        }
        // Use sh to write the file so that path resolution (e.g. /tmp on Windows/MSYS2)
        // is handled by the same shell that runs subsequent shell tasks, keeping paths
        // consistent across all local operations.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(write_pipe_command(dest, mode))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn sh for write to {}: {}", dest, e))?;
        {
            let stdin = child.stdin.take().ok_or("Failed to open stdin for write")?;
            let mut stdin = stdin;
            stdin
                .write_all(bytes)
                .map_err(|e| format!("Failed to write bytes to {}: {}", dest, e))?;
        }
        let status = child
            .wait()
            .map_err(|e| format!("Failed to wait on write process for {}: {}", dest, e))?;
        if !status.success() {
            return Err(
                format!("Failed to write {}: sh exited with status {}", dest, status).into(),
            );
        }
        Ok(())
    } else {
        let session = session.ok_or("write_to_target: remote target requires session")?;
        if become_enabled {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp_path = format!("/tmp/deploy-helper-{}-{}", nanos, std::process::id());

            let sftp = session
                .sftp()
                .map_err(|e| format!("Failed to open SFTP session: {}", e))?;
            {
                // With a mode, the staged copy in /tmp is created 0600 so the
                // content is never world-readable, not even before placement.
                let mut remote = if mode.is_some() {
                    sftp.open_mode(
                        Path::new(&tmp_path),
                        ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::TRUNCATE,
                        0o600,
                        ssh2::OpenType::File,
                    )
                } else {
                    sftp.create(Path::new(&tmp_path))
                }
                .map_err(|e| format!("Failed to write {}: {}", tmp_path, e))?;
                remote
                    .write_all(bytes)
                    .map_err(|e| format!("Failed to write {}: {}", tmp_path, e))?;
            }

            let inner = format!(
                "trap 'rm -f {tmp}' EXIT; {place}",
                tmp = shell_escape(&tmp_path),
                place = place_file_command(&tmp_path, dest, mode)
            );

            if let Some(password) = doas_pw {
                let doas_cmd = wrap_become_command(&inner, "doas", None);
                let (out, _stderr, code) =
                    execute_ssh_doas_with_pty(session, &doas_cmd, password, false, None, false)?;
                if code != 0 {
                    return Err(
                        format!("Failed to write {}: exit {}: {}", dest, code, out.trim()).into(),
                    );
                }
                return Ok(());
            }

            let wrapped = wrap_become_command(&inner, become_method, become_password);

            let (_stdout, stderr, code) =
                execute_ssh_command(session, &wrapped, true, false, None, false)?;
            if code != 0 {
                return Err(
                    format!("Failed to write {}: exit {}: {}", dest, code, stderr.trim()).into(),
                );
            }
            return Ok(());
        }
        let sftp = session
            .sftp()
            .map_err(|e| format!("Failed to open SFTP session: {}", e))?;
        if let Some(m) = mode {
            // Stage next to dest with 0600 via SFTP, then chmod to the exact
            // mode (SFTP create modes are subject to the server's umask) and
            // atomically mv into place.
            let dtmp = mode_dest_tmp(dest);
            {
                let mut remote = sftp
                    .open_mode(
                        Path::new(&dtmp),
                        ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::TRUNCATE,
                        0o600,
                        ssh2::OpenType::File,
                    )
                    .map_err(|e| format!("Failed to write {}: {}", dtmp, e))?;
                remote
                    .write_all(bytes)
                    .map_err(|e| format!("Failed to write {}: {}", dtmp, e))?;
            }
            let place = format!(
                "chmod {m} {dtmp} && mv -f {dtmp} {dst}",
                m = m,
                dtmp = shell_escape(&dtmp),
                dst = shell_escape(dest)
            );
            let (_stdout, stderr, code) =
                execute_ssh_command(session, &place, true, false, None, false)?;
            if code != 0 {
                return Err(
                    format!("Failed to write {}: exit {}: {}", dest, code, stderr.trim()).into(),
                );
            }
            return Ok(());
        }
        let mut remote = sftp
            .create(Path::new(dest))
            .map_err(|e| format!("Failed to write {}: {}", dest, e))?;
        remote
            .write_all(bytes)
            .map_err(|e| format!("Failed to write {}: {}", dest, e))?;
        Ok(())
    }
}

/// Recursively walk `base`, collecting remote directory paths and (local file, remote dest)
/// pairs. The CONTENTS of `base` are placed under `dest_dir` (like `cp -r base/. dest/`).
fn collect_dir_tree(
    base: &Path,
    cur: &Path,
    dest_dir: &str,
    dirs: &mut Vec<String>,
    files: &mut Vec<(PathBuf, String)>,
) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(cur)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap_or(path.as_path());
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let remote = format!("{}/{}", dest_dir.trim_end_matches('/'), rel_str);
        if path.is_dir() {
            dirs.push(remote.clone());
            collect_dir_tree(base, &path, dest_dir, dirs, files)?;
        } else {
            files.push((path, remote));
        }
    }
    Ok(())
}

/// Copy a local directory's CONTENTS into `dest_dir` on the target. Overlay semantics:
/// creates missing dirs, overwrites matching files, leaves unrelated files alone (never
/// deletes). Reuses write_to_target per file so become/SFTP handling is identical to a
/// single-file copy.
pub fn write_dir_to_target(
    src_dir: &Path,
    dest_dir: &str,
    is_localhost: bool,
    session: Option<&Session>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut dirs: Vec<String> = vec![dest_dir.trim_end_matches('/').to_string()];
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    collect_dir_tree(src_dir, src_dir, dest_dir, &mut dirs, &mut files)
        .map_err(|e| format!("Failed to read source dir {}: {}", src_dir.display(), e))?;

    // 1. Create the directory skeleton (one mkdir -p for all dirs; -p makes order
    // moot). Run it through the same execution paths the per-file writes use rather
    // than Rust's fs, so path resolution (e.g. /tmp on Windows/MSYS2) and become
    // handling stay identical: fs::create_dir_all would resolve /tmp to a different
    // place than the `sh` that writes the files, leaving the writes with no parent dir.
    let escaped: Vec<String> = dirs.iter().map(|d| shell_escape(d)).collect();
    let mkdir = format!("mkdir -p {}", escaped.join(" "));

    let (out, stderr, code) = run_shell_on_target(
        &mkdir,
        is_localhost,
        session,
        become_enabled,
        become_method,
        become_password,
    )?;

    if code != 0 {
        // The doas-PTY path merges stderr into stdout, so fall back to it when stderr is empty.
        let detail = if stderr.trim().is_empty() {
            out.trim()
        } else {
            stderr.trim()
        };
        return Err(format!("Failed to create dirs under {}: {}", dest_dir, detail).into());
    }

    // 2. Write each file through the shared single-file path.
    for (local, remote) in &files {
        let bytes =
            fs::read(local).map_err(|e| format!("Failed to read {}: {}", local.display(), e))?;
        write_to_target(
            &bytes,
            remote,
            is_localhost,
            session,
            become_enabled,
            become_method,
            become_password,
            None,
        )?;
    }

    println!(
        "{}",
        format!("  ({} files into {} dirs)", files.len(), dirs.len()).bright_black()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // shell_escape

    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn test_shell_escape_with_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn test_shell_escape_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_shell_escape_with_special_chars() {
        assert_eq!(shell_escape("a && b | c"), "'a && b | c'");
    }

    // render_env_values / env_export_lines

    fn env_map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_render_env_values_substitutes_placeholders() {
        let mut vars = IndexMap::new();
        vars.insert("b2_key".to_string(), Value::String("s3cret".to_string()));
        let env = env_map(&[("B2_APP_KEY", "{{ b2_key }}")]);
        let rendered = render_env_values(&env, &vars).unwrap();
        assert_eq!(rendered.get("B2_APP_KEY").unwrap(), "s3cret");
    }

    #[test]
    fn test_render_env_values_rejects_non_identifier_keys() {
        let vars = IndexMap::new();
        for key in ["BAD KEY", "1LEADING", "SEMI;COLON", ""] {
            let env = env_map(&[(key, "x")]);
            assert!(
                render_env_values(&env, &vars).is_err(),
                "should reject key '{}'",
                key
            );
        }
    }

    #[test]
    fn test_parse_env_output_preserves_literal_values() {
        let parsed = parse_env_output(
            "database_sha256=abc123\nfiles=42\nurl=https://example.com?a=b\nempty=\n",
        )
        .unwrap();
        assert_eq!(parsed["database_sha256"], "abc123");
        assert_eq!(parsed["files"], "42");
        assert_eq!(parsed["url"], "https://example.com?a=b");
        assert_eq!(parsed["empty"], "");
    }

    #[test]
    fn test_parse_env_output_ignores_blank_lines_and_comments() {
        let parsed = parse_env_output("# manifest\n\nstatus=healthy\n").unwrap();
        assert_eq!(parsed["status"], "healthy");
    }

    #[test]
    fn test_parse_env_output_rejects_invalid_and_repeated_keys() {
        assert_eq!(
            parse_env_output("valid=1\nnot valid=2").unwrap_err(),
            "line 2 has invalid key 'not valid'"
        );
        assert_eq!(
            parse_env_output("key=first\nkey=second").unwrap_err(),
            "line 2 repeats key 'key'"
        );
        assert_eq!(
            parse_env_output("valid=1\nmissing").unwrap_err(),
            "line 2 must be KEY=VALUE"
        );
    }

    #[test]
    fn test_structured_filter_names_in_plain_text_remain_strings() {
        let vars = IndexMap::new();
        for (template, expected) in [
            ("support from_json and from_env", "support from_json and from_env"),
            ("printf value | from_env", "printf value | from_env"),
            ("{{ 'printf value | from_env' }}", "printf value | from_env"),
        ] {
            assert_eq!(
                replace_placeholders_vars(template, &vars),
                Value::String(expected.to_string())
            );
        }
        assert!(uses_template_filter(
            "{{ manifest_output.stdout | from_env }}",
            "from_env"
        ));
    }

    #[test]
    fn test_env_export_lines_escape_values() {
        let rendered = env_map(&[("KEY", "it's a && b")]);
        assert_eq!(
            env_export_lines(&rendered),
            vec!["export KEY='it'\\''s a && b'"]
        );
    }

    // validate_mode

    #[test]
    fn test_validate_mode_accepts_octal() {
        for m in ["0600", "600", "755", "0755", "7", "1777"] {
            assert!(validate_mode(m).is_ok(), "should accept {}", m);
        }
    }

    #[test]
    fn test_validate_mode_rejects_non_octal() {
        for m in ["", "0800", "rw-r--r--", "u+x", "07777", "6 00"] {
            assert!(validate_mode(m).is_err(), "should reject {}", m);
        }
    }

    // staged-write command builders

    #[test]
    fn test_write_pipe_command_without_mode() {
        assert_eq!(write_pipe_command("/etc/app.env", None), "cat > '/etc/app.env'");
    }

    #[test]
    fn test_write_pipe_command_with_mode_stages_and_moves() {
        assert_eq!(
            write_pipe_command("/etc/app.env", Some("0600")),
            "umask 077 && cat > '/etc/app.env.deploy-helper-tmp' && chmod 0600 '/etc/app.env.deploy-helper-tmp' && mv -f '/etc/app.env.deploy-helper-tmp' '/etc/app.env'"
        );
    }

    #[test]
    fn test_place_file_command_without_mode() {
        assert_eq!(
            place_file_command("/tmp/stage", "/etc/app.env", None),
            "cp '/tmp/stage' '/etc/app.env'"
        );
    }

    #[test]
    fn test_place_file_command_with_mode_stages_and_moves() {
        assert_eq!(
            place_file_command("/tmp/stage", "/etc/app.env", Some("640")),
            "umask 077 && cp '/tmp/stage' '/etc/app.env.deploy-helper-tmp' && chmod 640 '/etc/app.env.deploy-helper-tmp' && mv -f '/etc/app.env.deploy-helper-tmp' '/etc/app.env'"
        );
    }

    // heredoc_delimiter

    #[test]
    fn test_heredoc_delimiter_single_quoted() {
        assert_eq!(
            heredoc_delimiter("cat << 'EOF' > file"),
            Some("EOF".to_string())
        );
    }

    #[test]
    fn test_heredoc_delimiter_double_quoted() {
        assert_eq!(heredoc_delimiter("cat << \"EOF\""), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_unquoted() {
        assert_eq!(heredoc_delimiter("cat << EOF"), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_dash() {
        assert_eq!(heredoc_delimiter("cat <<- 'EOF'"), Some("EOF".to_string()));
    }

    #[test]
    fn test_heredoc_delimiter_none() {
        assert_eq!(heredoc_delimiter("echo hello"), None);
    }

    // split_commands

    #[test]
    fn test_split_commands_single() {
        assert_eq!(split_commands("echo hello"), vec!["echo hello"]);
    }

    #[test]
    fn test_split_commands_multiple() {
        let input = "echo one\necho two\necho three";
        assert_eq!(
            split_commands(input),
            vec!["echo one", "echo two", "echo three"]
        );
    }

    #[test]
    fn test_split_commands_skips_empty_lines() {
        let input = "echo one\n\necho two";
        assert_eq!(split_commands(input), vec!["echo one", "echo two"]);
    }

    #[test]
    fn test_split_commands_line_continuation() {
        let input = "echo \\\none \\\ntwo";
        assert_eq!(split_commands(input), vec!["echo one two"]);
    }

    #[test]
    fn test_split_commands_heredoc_single_quoted() {
        let input = "cat << 'EOF' > /tmp/file\nline one\nline two\nEOF";
        assert_eq!(
            split_commands(input),
            vec!["cat << 'EOF' > /tmp/file\nline one\nline two\nEOF"]
        );
    }

    #[test]
    fn test_split_commands_heredoc_unquoted() {
        let input = "cat << EOF\ncontent\nEOF";
        assert_eq!(split_commands(input), vec!["cat << EOF\ncontent\nEOF"]);
    }

    #[test]
    fn test_split_commands_heredoc_then_command() {
        let input = "cat << 'EOF' > /tmp/file\ncontent\nEOF\necho done";
        assert_eq!(
            split_commands(input),
            vec!["cat << 'EOF' > /tmp/file\ncontent\nEOF", "echo done"]
        );
    }

    #[test]
    fn test_split_commands_heredoc_preserves_indentation() {
        let input = "cat << 'EOF' > /tmp/file\n    indented\n        more\nEOF";
        assert_eq!(
            split_commands(input),
            vec!["cat << 'EOF' > /tmp/file\n    indented\n        more\nEOF"]
        );
    }

    #[test]
    fn test_split_commands_if_block() {
        let input = "if [ -f /tmp/x ]; then\n  echo yes\nfi";
        assert_eq!(
            split_commands(input),
            vec!["if [ -f /tmp/x ]; then\necho yes\nfi"]
        );
    }

    #[test]
    fn test_split_commands_if_else_elif() {
        let input = "if a; then\n  x\nelif b; then\n  y\nelse\n  z\nfi";
        assert_eq!(
            split_commands(input),
            vec!["if a; then\nx\nelif b; then\ny\nelse\nz\nfi"]
        );
    }

    #[test]
    fn test_split_commands_for_loop() {
        let input = "for x in a b c; do\n  echo $x\ndone";
        assert_eq!(
            split_commands(input),
            vec!["for x in a b c; do\necho $x\ndone"]
        );
    }

    #[test]
    fn test_split_commands_while_loop() {
        let input = "while true; do\n  echo hi\ndone";
        assert_eq!(split_commands(input), vec!["while true; do\necho hi\ndone"]);
    }

    #[test]
    fn test_split_commands_until_loop() {
        let input = "until test -f /tmp/x; do\n  sleep 1\ndone";
        assert_eq!(
            split_commands(input),
            vec!["until test -f /tmp/x; do\nsleep 1\ndone"]
        );
    }

    #[test]
    fn test_split_commands_case_statement() {
        let input = "case $x in\n  a) echo A;;\n  b) echo B;;\nesac";
        assert_eq!(
            split_commands(input),
            vec!["case $x in\na) echo A;;\nb) echo B;;\nesac"]
        );
    }

    #[test]
    fn test_split_commands_nested_if() {
        let input = "if a; then\n  if b; then\n    c\n  fi\nfi";
        assert_eq!(
            split_commands(input),
            vec!["if a; then\nif b; then\nc\nfi\nfi"]
        );
    }

    #[test]
    fn test_split_commands_compound_then_next_command() {
        let input = "if foo; then\n  bar\nfi\necho done";
        assert_eq!(
            split_commands(input),
            vec!["if foo; then\nbar\nfi", "echo done"]
        );
    }

    #[test]
    fn test_split_commands_compound_on_one_line() {
        let input = "if foo; then bar; fi\necho next";
        assert_eq!(
            split_commands(input),
            vec!["if foo; then bar; fi", "echo next"]
        );
    }

    #[test]
    fn test_split_commands_keyword_as_argument() {
        let input = "echo for\necho done";
        assert_eq!(split_commands(input), vec!["echo for", "echo done"]);
    }

    #[test]
    fn test_split_commands_keyword_in_single_quotes() {
        let input = "echo 'if foo'\necho next";
        assert_eq!(split_commands(input), vec!["echo 'if foo'", "echo next"]);
    }

    #[test]
    fn test_split_commands_keyword_in_double_quotes() {
        let input = "echo \"if foo\"\necho next";
        assert_eq!(split_commands(input), vec!["echo \"if foo\"", "echo next"]);
    }

    #[test]
    fn test_split_commands_if_with_heredoc_inside() {
        let input = "if foo; then\n  cat << EOF\nhello\nEOF\nfi";
        assert_eq!(
            split_commands(input),
            vec!["if foo; then\ncat << EOF\nhello\nEOF\nfi"]
        );
    }

    #[test]
    fn test_split_commands_comment_with_keyword() {
        let input = "echo hi # if this were a thing\necho bye";
        assert_eq!(
            split_commands(input),
            vec!["echo hi # if this were a thing", "echo bye"]
        );
    }

    #[test]
    fn test_split_commands_dns_record_block() {
        let input = "existing=$(curl -s ... | grep -o '\"name\":\"sub\"' || true)\nif [ -z \"$existing\" ]; then\n  curl -X POST ...\n  sleep 30\nfi";
        assert_eq!(
            split_commands(input),
            vec![
                "existing=$(curl -s ... | grep -o '\"name\":\"sub\"' || true)",
                "if [ -z \"$existing\" ]; then\ncurl -X POST ...\nsleep 30\nfi"
            ]
        );
    }

    // doas output cleanup

    #[test]
    fn test_strip_doas_password_prompt_preserves_command_whitespace() {
        let output = "\rdoas (user@host) password: \r\n  root";
        assert_eq!(strip_doas_password_prompt(output), "  root");
    }

    #[test]
    fn test_strip_doas_password_prompt_leaves_normal_output_unchanged() {
        assert_eq!(strip_doas_password_prompt("root"), "root");
    }

    // wrap_become_command

    #[test]
    fn test_wrap_become_sudo_with_password() {
        let result =
            wrap_become_command("nginx -t && systemctl reload nginx", "sudo", Some("secret"));
        assert_eq!(
            result,
            "printf '%s\\n' 'secret' | sudo -S -p '' sh -c 'nginx -t && systemctl reload nginx'"
        );
    }

    #[test]
    fn test_wrap_become_sudo_nopasswd() {
        let result = wrap_become_command("nginx -t", "sudo", None);
        assert_eq!(result, "sudo sh -c 'nginx -t'");
    }

    #[test]
    fn test_wrap_become_doas_nopasswd() {
        let result = wrap_become_command("nginx -t", "doas", None);
        assert_eq!(result, "doas sh -c 'nginx -t'");
    }

    #[test]
    fn test_wrap_become_password_with_special_chars() {
        let result = wrap_become_command("id", "sudo", Some("p@ss'word"));
        assert_eq!(
            result,
            "printf '%s\\n' 'p@ss'\\''word' | sudo -S -p '' sh -c 'id'"
        );
    }

    #[test]
    fn test_wrap_become_su_with_password() {
        let result = wrap_become_command("nginx -t", "su", Some("secret"));
        assert_eq!(result, "printf '%s\\n' 'secret' | su -c 'nginx -t'");
    }

    #[test]
    fn test_wrap_become_su_nopasswd() {
        let result = wrap_become_command("nginx -t", "su", None);
        assert_eq!(result, "su -c 'nginx -t'");
    }

    // annotate_yaml_error

    #[test]
    fn test_annotate_yaml_error_unquoted_template() {
        let contents = "- name: x\n  chdir: {{ app_path }}\n";
        #[derive(serde::Deserialize, Debug)]
        struct S {
            #[allow(dead_code)]
            name: String,
            #[allow(dead_code)]
            chdir: String,
        }
        let err = serde_yaml::from_str::<Vec<S>>(contents).unwrap_err();
        let out = annotate_yaml_error("setup.yml", contents, err);
        assert!(
            out.contains("unquoted"),
            "missing plain-language hint in: {}",
            out
        );
        assert!(out.contains("line 2"), "missing line number in: {}", out);
        assert!(
            out.contains("chdir: {{ app_path }}"),
            "missing source line in: {}",
            out
        );
        assert!(
            !out.contains("invalid type: map"),
            "leaks raw serde_yaml jargon: {}",
            out
        );
    }

    #[test]
    fn test_annotate_yaml_error_passthrough_when_not_map_error() {
        let contents = "- name: x\n  chdir: [invalid\n";
        let err = serde_yaml::from_str::<serde_yaml::Value>(contents).unwrap_err();
        let out = annotate_yaml_error("setup.yml", contents, err);
        assert!(!out.contains("unquoted"), "should not add hint: {}", out);
    }

    #[test]
    fn test_annotate_yaml_error_no_hint_when_line_not_templated() {
        let contents = "- name: x\n  chdir: {key: val}\n";
        #[derive(serde::Deserialize, Debug)]
        struct S {
            #[allow(dead_code)]
            name: String,
            #[allow(dead_code)]
            chdir: String,
        }
        let err = serde_yaml::from_str::<Vec<S>>(contents).unwrap_err();
        let out = annotate_yaml_error("setup.yml", contents, err);
        assert!(!out.contains("unquoted"), "should not add hint: {}", out);
    }

    // resolve_src_path

    #[test]
    fn test_resolve_src_path_relative() {
        let dir = Path::new("/some/deploy");
        let resolved = resolve_src_path(dir, "templates/x.j2");
        assert_eq!(resolved, PathBuf::from("/some/deploy/templates/x.j2"));
    }

    #[test]
    fn test_resolve_src_path_absolute_passes_through() {
        let dir = Path::new("/some/deploy");
        let resolved = resolve_src_path(dir, "/etc/x.conf");
        assert_eq!(resolved, PathBuf::from("/etc/x.conf"));
    }

    // collect_dir_tree — uses Rust's own fs to build/read the tree so the test is
    // portable (no shell, no /tmp resolution differences).

    fn scratch_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("dh-cdt-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        base
    }

    #[test]
    fn test_collect_dir_tree_maps_nested_paths() {
        let base = scratch_dir("nested");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("a.txt"), b"a").unwrap();
        fs::write(base.join("sub/b.txt"), b"b").unwrap();

        // Mirror write_dir_to_target: the dest dir itself seeds `dirs`.
        let mut dirs = vec!["/dest".to_string()];
        let mut files = Vec::new();
        collect_dir_tree(&base, &base, "/dest", &mut dirs, &mut files).unwrap();

        assert_eq!(dirs, vec!["/dest".to_string(), "/dest/sub".to_string()]);
        let remote: Vec<&str> = files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(remote, vec!["/dest/a.txt", "/dest/sub/b.txt"]);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn test_collect_dir_tree_empty_dir_yields_no_files() {
        let base = scratch_dir("empty");
        fs::create_dir_all(&base).unwrap();

        let mut dirs = vec!["/dest".to_string()];
        let mut files = Vec::new();
        collect_dir_tree(&base, &base, "/dest", &mut dirs, &mut files).unwrap();

        assert_eq!(dirs, vec!["/dest".to_string()]);
        assert!(files.is_empty());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn test_collect_dir_tree_trims_trailing_slash_in_dest() {
        let base = scratch_dir("slash");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("a.txt"), b"a").unwrap();

        let mut dirs = Vec::new();
        let mut files = Vec::new();
        collect_dir_tree(&base, &base, "/dest/", &mut dirs, &mut files).unwrap();

        let remote: Vec<&str> = files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(remote, vec!["/dest/a.txt"]); // not "/dest//a.txt"

        fs::remove_dir_all(&base).unwrap();
    }
}
