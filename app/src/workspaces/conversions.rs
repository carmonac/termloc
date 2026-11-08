use std::path::PathBuf;

use anyhow::anyhow;
use warp_errors::report_error;

use crate::settings::AgentModeCommandExecutionPredicate;

pub(crate) trait ToAgentModeCommandExecutionPredicates {
    fn to_predicates(self) -> Vec<AgentModeCommandExecutionPredicate>;
}

impl ToAgentModeCommandExecutionPredicates for Vec<String> {
    fn to_predicates(self) -> Vec<AgentModeCommandExecutionPredicate> {
        self.into_iter()
            .filter_map(
                |pattern| match AgentModeCommandExecutionPredicate::new_regex(&pattern) {
                    Ok(predicate) => Some(predicate),
                    Err(e) => {
                        report_error!(anyhow!(e).context(
                            "Couldn't parse command regex into AgentModeCommandExecutionPredicate"
                        ));
                        None
                    }
                },
            )
            .collect()
    }
}

pub(crate) trait ToPathBufs {
    fn to_path_bufs(self) -> Vec<PathBuf>;
}

impl ToPathBufs for Vec<String> {
    fn to_path_bufs(self) -> Vec<PathBuf> {
        self.into_iter().map(PathBuf::from).collect()
    }
}
