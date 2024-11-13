use indexmap::IndexMap;
use serde_json::Value;

use crate::utils;

pub fn process(condition: &Option<String>, vars_map: &IndexMap<String, Value>) -> bool {
    if let Some(cond) = condition {
        let template_str = format!("{{% if {} %}}true{{% else %}}false{{% endif %}}", cond);
        let rendered_cond = utils::replace_placeholders(&template_str, vars_map);
        if rendered_cond == "false" {
            false
        } else {
            true
        }
    } else {
        true
    }
}
