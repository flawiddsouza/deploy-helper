use colored::Colorize;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use ssh2::Session;
use std::thread;
use std::time::{Duration, Instant};

use crate::common::{Register, VerifyExpectation, VerifySpec};
use crate::utils;

#[derive(Debug)]
enum Matcher {
    Equals(String),
    Regex { source: String, compiled: Regex },
}

#[derive(Debug)]
struct VerifyResolved {
    command: String,
    matcher: Option<Matcher>,
    attempts: u32,
    delay_seconds: u64,
    max_elapsed_seconds: Option<u64>,
}

fn resolve(
    task_name: &str,
    spec: &VerifySpec,
    vars_map: &IndexMap<String, Value>,
    no_log: bool,
) -> Result<VerifyResolved, Box<dyn std::error::Error>> {
    let command = utils::replace_placeholders(&spec.command, vars_map);
    if command.trim().is_empty() {
        return Err(format!("Task '{}': verify command must not be empty", task_name).into());
    }

    let matcher = match &spec.expect {
        None => None,
        Some(VerifyExpectation {
            equals: Some(expected),
            regex: None,
        }) => Some(Matcher::Equals(utils::replace_placeholders(
            expected, vars_map,
        ))),
        Some(VerifyExpectation {
            equals: None,
            regex: Some(pattern),
        }) => {
            let source = utils::replace_placeholders(pattern, vars_map);
            let compiled = Regex::new(&source).map_err(|e| {
                if no_log {
                    format!(
                        "Task '{}': verify expect.regex is invalid (details hidden by no_log)",
                        task_name
                    )
                } else {
                    format!(
                        "Task '{}': verify expect.regex is invalid: {}",
                        task_name, e
                    )
                }
            })?;
            Some(Matcher::Regex { source, compiled })
        }
        Some(_) => {
            return Err(format!(
                "Task '{}': verify expect must set exactly one of equals or regex",
                task_name
            )
            .into())
        }
    };

    let (attempts, delay_seconds, max_elapsed_seconds) = spec
        .retry
        .as_ref()
        .map(|retry| {
            (
                retry.attempts,
                retry.delay_seconds,
                retry.max_elapsed_seconds,
            )
        })
        .unwrap_or((1, 0, None));
    if attempts == 0 {
        return Err(format!(
            "Task '{}': verify retry.attempts must be at least 1",
            task_name
        )
        .into());
    }
    if max_elapsed_seconds == Some(0) {
        return Err(format!(
            "Task '{}': verify retry.max_elapsed_seconds must be at least 1",
            task_name
        )
        .into());
    }

    Ok(VerifyResolved {
        command,
        matcher,
        attempts,
        delay_seconds,
        max_elapsed_seconds,
    })
}

fn retry_wait(
    delay_seconds: u64,
    max_elapsed_seconds: Option<u64>,
    elapsed: Duration,
) -> Option<Duration> {
    let delay = Duration::from_secs(delay_seconds);
    let Some(max_elapsed_seconds) = max_elapsed_seconds else {
        return Some(delay);
    };
    let remaining = Duration::from_secs(max_elapsed_seconds).checked_sub(elapsed)?;
    if remaining.is_zero() || (!delay.is_zero() && remaining <= delay) {
        None
    } else {
        Some(delay)
    }
}

fn mismatch(matcher: Option<&Matcher>, actual: &str) -> Option<String> {
    match matcher {
        None => None,
        Some(Matcher::Equals(expected)) if actual != expected => Some(format!(
            "expected stdout {}, got {}",
            quoted(expected),
            quoted(actual)
        )),
        Some(Matcher::Regex { source, compiled }) if !compiled.is_match(actual) => Some(format!(
            "expected stdout to match regex {}, got {}",
            quoted(source),
            quoted(actual)
        )),
        Some(_) => None,
    }
}

fn quoted(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\'', "\\'");
    format!("'{}'", escaped)
}

#[allow(clippy::too_many_arguments)]
pub fn process(
    task_name: &str,
    spec: &VerifySpec,
    environment: Option<&IndexMap<String, String>>,
    is_localhost: bool,
    session: Option<&Session>,
    task_chdir: Option<&str>,
    register: Option<&String>,
    login_shell: bool,
    vars_map: &mut IndexMap<String, Value>,
    become_enabled: bool,
    become_method: &str,
    become_password: Option<&str>,
    no_log: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve(task_name, spec, vars_map, no_log)?;
    let environment_resolved = match environment {
        Some(environment) if !environment.is_empty() => {
            Some(utils::render_env_values(environment, vars_map)?)
        }
        _ => None,
    };

    if !no_log {
        println!("{}", format!("> [verify] {}", resolved.command).magenta());
    }

    let mut failure = String::new();
    let started = Instant::now();
    let mut attempts_run = 0;
    let mut elapsed_limit_reached = false;
    for attempt in 1..=resolved.attempts {
        attempts_run = attempt;
        match utils::run_shell_on_target_with_context(
            &resolved.command,
            is_localhost,
            session,
            become_enabled,
            become_method,
            become_password,
            task_chdir,
            login_shell,
            environment_resolved.as_ref(),
        ) {
            Ok((stdout, stderr, rc)) => {
                if rc != 0 {
                    let detail = if stderr.trim().is_empty() {
                        stdout.trim()
                    } else {
                        stderr.trim()
                    };
                    failure = if detail.is_empty() {
                        format!("command exited with status {}", rc)
                    } else {
                        format!("command exited with status {}: {}", rc, detail)
                    };
                } else if let Some(detail) = mismatch(resolved.matcher.as_ref(), &stdout) {
                    failure = detail;
                } else {
                    if let Some(register) = register {
                        let value = serde_json::to_value(Register { stdout, stderr, rc })?;
                        vars_map.insert(register.clone(), value);
                        println!(
                            "{}",
                            format!("Registering output to: {}", register).yellow()
                        );
                    }
                    return Ok(());
                }
            }
            Err(error) => failure = format!("command execution failed: {}", error),
        }

        if attempt < resolved.attempts {
            let Some(wait) = retry_wait(
                resolved.delay_seconds,
                resolved.max_elapsed_seconds,
                started.elapsed(),
            ) else {
                elapsed_limit_reached = true;
                break;
            };
            if !no_log {
                println!(
                    "{}",
                    format!(
                        "Verification attempt {}/{} failed; retrying in {} second{}",
                        attempt,
                        resolved.attempts,
                        wait.as_secs(),
                        if wait == Duration::from_secs(1) {
                            ""
                        } else {
                            "s"
                        }
                    )
                    .yellow()
                );
            }
            if !wait.is_zero() {
                thread::sleep(wait);
            }
            if resolved
                .max_elapsed_seconds
                .is_some_and(|max| started.elapsed() >= Duration::from_secs(max))
            {
                elapsed_limit_reached = true;
                break;
            }
        }
    }

    let elapsed_limit_detail = if elapsed_limit_reached {
        format!(
            " (retry elapsed time limit of {} second{} prevented further retries)",
            resolved.max_elapsed_seconds.unwrap(),
            if resolved.max_elapsed_seconds == Some(1) {
                ""
            } else {
                "s"
            }
        )
    } else {
        String::new()
    };

    if no_log {
        Err(format!(
            "Task '{}': verification failed after {} attempt{}{} (details hidden by no_log)",
            task_name,
            attempts_run,
            if attempts_run == 1 { "" } else { "s" },
            elapsed_limit_detail
        )
        .into())
    } else {
        Err(format!(
            "Task '{}': verification failed after {} attempt{}{}: {}",
            task_name,
            attempts_run,
            if attempts_run == 1 { "" } else { "s" },
            elapsed_limit_detail,
            failure
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::{mismatch, resolve, retry_wait, Matcher};
    use crate::common::{VerifyExpectation, VerifyRetry, VerifySpec};
    use indexmap::IndexMap;
    use regex::Regex;
    use std::time::Duration;

    fn spec(expect: Option<VerifyExpectation>) -> VerifySpec {
        VerifySpec {
            command: "echo healthy".to_string(),
            expect,
            retry: None,
        }
    }

    #[test]
    fn equals_requires_exact_output() {
        let matcher = Matcher::Equals("healthy".to_string());
        assert!(mismatch(Some(&matcher), "healthy").is_none());
        assert!(mismatch(Some(&matcher), " healthy").is_some());
    }

    #[test]
    fn regex_matches_command_output() {
        let matcher = Matcher::Regex {
            source: "^health(y|ier)$".to_string(),
            compiled: Regex::new("^health(y|ier)$").unwrap(),
        };
        assert!(mismatch(Some(&matcher), "healthy").is_none());
        assert!(mismatch(Some(&matcher), "unhealthy").is_some());
    }

    #[test]
    fn resolve_rejects_both_matchers() {
        let verify = spec(Some(VerifyExpectation {
            equals: Some("healthy".to_string()),
            regex: Some("healthy".to_string()),
        }));
        let err = resolve("Example", &verify, &IndexMap::new(), false).unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn resolve_rejects_invalid_regex() {
        let verify = spec(Some(VerifyExpectation {
            equals: None,
            regex: Some("[".to_string()),
        }));
        let err = resolve("Example", &verify, &IndexMap::new(), false).unwrap_err();
        assert!(err.to_string().contains("expect.regex is invalid"));
    }

    #[test]
    fn resolve_hides_invalid_regex_details_with_no_log() {
        let verify = spec(Some(VerifyExpectation {
            equals: None,
            regex: Some("VERIFY_SUPER_SECRET[".to_string()),
        }));
        let err = resolve("Example", &verify, &IndexMap::new(), true).unwrap_err();
        assert!(err.to_string().contains("details hidden by no_log"));
        assert!(!err.to_string().contains("VERIFY_SUPER_SECRET"));
    }

    #[test]
    fn resolve_rejects_zero_attempts() {
        let mut verify = spec(None);
        verify.retry = Some(VerifyRetry {
            attempts: 0,
            delay_seconds: 0,
            max_elapsed_seconds: None,
        });
        let err = resolve("Example", &verify, &IndexMap::new(), false).unwrap_err();
        assert!(err.to_string().contains("must be at least 1"));
    }

    #[test]
    fn resolve_rejects_zero_max_elapsed_seconds() {
        let mut verify = spec(None);
        verify.retry = Some(VerifyRetry {
            attempts: 2,
            delay_seconds: 0,
            max_elapsed_seconds: Some(0),
        });
        let err = resolve("Example", &verify, &IndexMap::new(), false).unwrap_err();
        assert!(err
            .to_string()
            .contains("max_elapsed_seconds must be at least 1"));
    }

    #[test]
    fn retry_wait_stops_at_the_elapsed_time_limit() {
        assert_eq!(retry_wait(5, Some(12), Duration::from_secs(10)), None);
        assert_eq!(retry_wait(5, Some(12), Duration::from_secs(12)), None);
        assert_eq!(
            retry_wait(5, Some(12), Duration::from_secs(6)),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            retry_wait(5, None, Duration::from_secs(100)),
            Some(Duration::from_secs(5))
        );
    }
}
