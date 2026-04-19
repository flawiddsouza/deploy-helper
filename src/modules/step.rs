use std::io::{self, BufRead, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepChoice {
    Run,
    Skip,
    ContinueWithoutPrompt,
}

#[derive(Debug, Default)]
pub struct StepState {
    pub enabled: bool,
    pub continue_in_deployment: bool,
}

impl StepState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            continue_in_deployment: false,
        }
    }

    pub fn reset_for_deployment(&mut self) {
        self.continue_in_deployment = false;
    }

    pub fn should_prompt(&self) -> bool {
        self.enabled && !self.continue_in_deployment
    }
}

pub fn parse_choice(input: &str) -> Option<StepChoice> {
    match input.trim().to_ascii_lowercase().as_str() {
        "" | "n" => Some(StepChoice::Skip),
        "y" => Some(StepChoice::Run),
        "c" => Some(StepChoice::ContinueWithoutPrompt),
        _ => None,
    }
}

pub fn prompt(task_name: &str) -> io::Result<StepChoice> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    loop {
        print!("Perform task: {} (N)o/(y)es/(c)ontinue: ", task_name);
        stdout.lock().flush()?;

        let mut line = String::new();
        let bytes = stdin.lock().read_line(&mut line)?;
        if bytes == 0 {
            // EOF: treat as Skip
            return Ok(StepChoice::Skip);
        }
        if let Some(choice) = parse_choice(&line) {
            return Ok(choice);
        }
        // unknown input: reprompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_skip() {
        assert_eq!(parse_choice(""), Some(StepChoice::Skip));
        assert_eq!(parse_choice("\n"), Some(StepChoice::Skip));
    }

    #[test]
    fn n_is_skip() {
        assert_eq!(parse_choice("n"), Some(StepChoice::Skip));
        assert_eq!(parse_choice("N\n"), Some(StepChoice::Skip));
    }

    #[test]
    fn y_is_run() {
        assert_eq!(parse_choice("y"), Some(StepChoice::Run));
        assert_eq!(parse_choice("Y"), Some(StepChoice::Run));
    }

    #[test]
    fn c_is_continue() {
        assert_eq!(parse_choice("c"), Some(StepChoice::ContinueWithoutPrompt));
    }

    #[test]
    fn unknown_returns_none() {
        assert_eq!(parse_choice("maybe"), None);
        assert_eq!(parse_choice("1"), None);
    }

    #[test]
    fn should_prompt_follows_enabled_and_continue_flag() {
        let mut s = StepState::new(true);
        assert!(s.should_prompt());
        s.continue_in_deployment = true;
        assert!(!s.should_prompt());
        s.reset_for_deployment();
        assert!(s.should_prompt());
    }

    #[test]
    fn disabled_never_prompts() {
        let s = StepState::new(false);
        assert!(!s.should_prompt());
    }
}
