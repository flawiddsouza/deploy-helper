use indexmap::IndexMap;
use serde_json::Value;
use std::fmt;

use crate::utils;

struct WhenConditionError {
    source: minijinja::Error,
    condition: Option<String>,
}

impl fmt::Display for WhenConditionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.condition {
            Some(condition) => write!(
                formatter,
                "when condition failed: {} (when: {})",
                self.source, condition
            ),
            None => formatter.write_str("when condition failed (details hidden by no_log)"),
        }
    }
}

impl fmt::Debug for WhenConditionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for WhenConditionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn condition_error(
    source: minijinja::Error,
    condition: &str,
    no_log: bool,
) -> Box<dyn std::error::Error> {
    Box::new(WhenConditionError {
        source,
        condition: (!no_log).then(|| condition.to_string()),
    })
}

pub fn process(
    condition: &Option<String>,
    vars_map: &IndexMap<String, Value>,
    no_log: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let Some(condition) = condition else {
        return Ok(true);
    };

    let env = utils::template_environment();
    let condition_template = format!("{{% if {condition} %}}true{{% else %}}false{{% endif %}}");
    env.render_str(&condition_template, vars_map)
        .map(|value| value == "true")
        .map_err(|error| condition_error(error, condition, no_log))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    fn process_condition(
        condition: &str,
        vars_map: &IndexMap<String, Value>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        process(&Some(condition.to_string()), vars_map, false)
    }

    #[test]
    fn undefined_comparisons_are_errors() {
        let vars = IndexMap::new();
        for condition in [
            "missing != \"\"",
            "missing == \"\"",
            "missing < 1",
            "missing not in []",
        ] {
            let error = process_condition(condition, &vars).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("when condition failed: undefined value"));
            assert!(message.contains(&format!("when: {condition}")));
        }
    }

    #[test]
    fn top_level_undefined_conditions_are_errors() {
        let vars = IndexMap::from([("config".to_string(), serde_json::json!({}))]);
        for condition in ["missing", "config.token", "[] | first"] {
            let error = process_condition(condition, &vars).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("when condition failed: undefined value"),
                "condition should fail: {condition}"
            );
        }
    }

    #[test]
    fn nested_and_generated_undefined_comparisons_are_errors() {
        let vars = IndexMap::from([
            ("config".to_string(), serde_json::json!({})),
            ("scalar".to_string(), serde_json::json!("value")),
            ("items".to_string(), serde_json::json!(["first"])),
        ]);

        for condition in [
            "config.token != \"\"",
            "config[\"token\"] != \"\"",
            "scalar.token != \"\"",
            "items[1] != \"\"",
            "([] | first) != \"\"",
        ] {
            assert!(
                process_condition(condition, &vars).is_err(),
                "condition should fail: {condition}"
            );
        }
    }

    #[test]
    fn definedness_checks_and_defaults_keep_their_jinja_semantics() {
        let vars = IndexMap::new();
        assert!(!process_condition("missing is defined", &vars).unwrap());
        assert!(process_condition("missing is not defined", &vars).unwrap());
        assert!(process_condition("missing is undefined", &vars).unwrap());
        assert!(!process_condition("missing is defined and missing != \"\"", &vars).unwrap());
        assert!(process_condition("missing is not defined or missing != \"\"", &vars).unwrap());
        assert!(
            process_condition("missing | default(\"fallback\") == \"fallback\"", &vars).unwrap()
        );
    }

    #[test]
    fn native_map_and_sequence_behavior_is_preserved() {
        let vars = IndexMap::from([(
            "config".to_string(),
            serde_json::json!({"present": 1, "items": ["first"]}),
        )]);

        assert!(process_condition("\"present\" in config", &vars).unwrap());
        assert!(!process_condition("\"missing\" in config", &vars).unwrap());
        assert!(process_condition("\"missing\" not in config", &vars).unwrap());
        assert!(
            process_condition("config == {\"present\": 1, \"items\": [\"first\"]}", &vars).unwrap()
        );
        assert!(process_condition("config.items[0] == \"first\"", &vars).unwrap());
        assert!(process_condition("config | length == 2", &vars).unwrap());
    }

    #[test]
    fn no_log_hides_condition_errors() {
        let secret = "secret-only-in-when";
        let error = process(
            &condition(&format!("missing != \"{secret}\"")),
            &IndexMap::new(),
            true,
        )
        .unwrap_err();
        let message = error.to_string();
        assert_eq!(message, "when condition failed (details hidden by no_log)");
        assert!(!message.contains(secret));
    }
}
