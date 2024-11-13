use colored::Colorize;
use indexmap::IndexMap;
use serde_json::Value;

use crate::common::Debug;
use crate::utils;

pub fn process(debug: &Debug, vars_map: &IndexMap<String, Value>) {
    println!("{}", "Debug:".blue());
    for (key, msg) in debug.0.iter() {
        println!("{}", format!("{}:", key).blue());
        let debug_msg = utils::replace_placeholders(msg, vars_map);
        println!("{}", format!("{}", debug_msg).blue());
    }
}
