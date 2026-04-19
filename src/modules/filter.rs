use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct FilterConfig {
    pub tags: Vec<String>,
    pub skip_tags: Vec<String>,
    pub start_at_task: Option<String>,
}

#[derive(Debug, Default)]
pub struct GateState {
    pub started: bool,
}

impl GateState {
    pub fn new(config: &FilterConfig) -> Self {
        Self {
            started: config.start_at_task.is_none(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Run,
    Skip(SkipReason),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SkipReason {
    BeforeStart,
    AlwaysSkipped,
    Never,
    NoMatchingTag,
    SkipTag,
}

pub fn merge_tags(ancestors: &[String], own: Option<&[String]>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for t in ancestors.iter().chain(own.into_iter().flatten()) {
        if seen.insert(t.clone()) {
            out.push(t.clone());
        }
    }
    out
}

pub fn decide(
    task_name: &str,
    effective_tags: &[String],
    config: &FilterConfig,
    state: &mut GateState,
) -> Decision {
    // Gate 1: start-at-task positional skip
    if !state.started {
        if let Some(start_name) = config.start_at_task.as_deref() {
            if task_name == start_name {
                state.started = true;
            } else {
                return Decision::Skip(SkipReason::BeforeStart);
            }
        }
    }

    let has = |t: &str| effective_tags.iter().any(|x| x == t);
    let in_list = |list: &[String], t: &str| list.iter().any(|x| x == t);

    // Gate 2: always short-circuits everything except --skip-tags always
    if has("always") {
        if in_list(&config.skip_tags, "always") {
            return Decision::Skip(SkipReason::AlwaysSkipped);
        }
        return Decision::Run;
    }

    // Gate 3: never hides unless user explicitly opts in
    if has("never") {
        let opted_in = in_list(&config.tags, "never")
            || effective_tags
                .iter()
                .any(|t| t != "never" && in_list(&config.tags, t));
        if !opted_in {
            return Decision::Skip(SkipReason::Never);
        }
    }

    // Gate 4: --tags filter
    if !config.tags.is_empty() {
        let any_match = effective_tags.iter().any(|t| in_list(&config.tags, t));
        if !any_match {
            return Decision::Skip(SkipReason::NoMatchingTag);
        }
    }

    // Gate 5: --skip-tags filter
    if effective_tags.iter().any(|t| in_list(&config.skip_tags, t)) {
        return Decision::Skip(SkipReason::SkipTag);
    }

    Decision::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> FilterConfig {
        FilterConfig::default()
    }

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn merge_dedupes_and_preserves_order() {
        let ancestors = tags(&["web", "nginx"]);
        let own = tags(&["nginx", "tls"]);
        let merged = merge_tags(&ancestors, Some(&own));
        assert_eq!(merged, tags(&["web", "nginx", "tls"]));
    }

    #[test]
    fn merge_handles_none_own() {
        let ancestors = tags(&["web"]);
        let merged = merge_tags(&ancestors, None);
        assert_eq!(merged, tags(&["web"]));
    }

    #[test]
    fn no_config_runs_everything() {
        let c = cfg();
        let mut s = GateState::new(&c);
        assert_eq!(decide("t", &tags(&["a"]), &c, &mut s), Decision::Run);
        assert_eq!(decide("t", &[], &c, &mut s), Decision::Run);
    }

    #[test]
    fn tags_filter_matches_one_of_many() {
        let c = FilterConfig {
            tags: tags(&["web"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["web", "nginx"]), &c, &mut s),
            Decision::Run
        );
    }

    #[test]
    fn tags_filter_skips_when_no_match() {
        let c = FilterConfig {
            tags: tags(&["web"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["nginx"]), &c, &mut s),
            Decision::Skip(SkipReason::NoMatchingTag)
        );
    }

    #[test]
    fn skip_tags_wins_over_tags() {
        let c = FilterConfig {
            tags: tags(&["web"]),
            skip_tags: tags(&["tls"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["web", "tls"]), &c, &mut s),
            Decision::Skip(SkipReason::SkipTag)
        );
    }

    #[test]
    fn always_bypasses_tags_filter() {
        let c = FilterConfig {
            tags: tags(&["other"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["always", "nginx"]), &c, &mut s),
            Decision::Run
        );
    }

    #[test]
    fn always_bypasses_skip_tags_except_self() {
        let c = FilterConfig {
            skip_tags: tags(&["nginx"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["always", "nginx"]), &c, &mut s),
            Decision::Run
        );

        let c2 = FilterConfig {
            skip_tags: tags(&["always"]),
            ..cfg()
        };
        let mut s2 = GateState::new(&c2);
        assert_eq!(
            decide("t", &tags(&["always", "nginx"]), &c2, &mut s2),
            Decision::Skip(SkipReason::AlwaysSkipped)
        );
    }

    #[test]
    fn never_skipped_by_default() {
        let c = cfg();
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["never", "nuke"]), &c, &mut s),
            Decision::Skip(SkipReason::Never)
        );
    }

    #[test]
    fn never_opts_in_via_other_tag() {
        let c = FilterConfig {
            tags: tags(&["nuke"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("t", &tags(&["never", "nuke"]), &c, &mut s),
            Decision::Run
        );
    }

    #[test]
    fn never_opts_in_via_never_name() {
        let c = FilterConfig {
            tags: tags(&["never"]),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(decide("t", &tags(&["never"]), &c, &mut s), Decision::Run);
    }

    #[test]
    fn start_at_task_skips_until_name_match() {
        let c = FilterConfig {
            start_at_task: Some("middle".to_string()),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("first", &tags(&["a"]), &c, &mut s),
            Decision::Skip(SkipReason::BeforeStart)
        );
        assert_eq!(decide("middle", &tags(&["a"]), &c, &mut s), Decision::Run);
        assert_eq!(decide("last", &tags(&["a"]), &c, &mut s), Decision::Run);
    }

    #[test]
    fn start_at_task_skips_always_tasks_before_match() {
        let c = FilterConfig {
            start_at_task: Some("start".to_string()),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        assert_eq!(
            decide("before", &tags(&["always"]), &c, &mut s),
            Decision::Skip(SkipReason::BeforeStart)
        );
        assert_eq!(decide("start", &tags(&[]), &c, &mut s), Decision::Run);
    }

    #[test]
    fn start_at_task_stays_started_across_calls() {
        let c = FilterConfig {
            start_at_task: Some("go".to_string()),
            ..cfg()
        };
        let mut s = GateState::new(&c);
        let _ = decide("go", &[], &c, &mut s);
        assert!(s.started);
        assert_eq!(decide("later", &[], &c, &mut s), Decision::Run);
    }
}
