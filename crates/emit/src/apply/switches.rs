//! Switch edges permitted by the selector's known bits and wire identities.

use bennett_ir::{SwitchCase, Target};

use super::bits::Bit;
use super::selectors::Selector;

#[derive(Clone, Default)]
pub(super) struct Edges<'a> {
    pub(super) cases: Vec<&'a SwitchCase>,
    pub(super) default: Option<&'a Target>,
}

impl<'a> Edges<'a> {
    pub(super) fn new(cases: &'a [SwitchCase], default: &'a Target) -> Self {
        Self { cases: cases.iter().collect(), default: Some(default) }
    }

    pub(super) fn retain_possible(&mut self, selector: &[Bit]) {
        let facts = Selector::new(selector);
        self.cases.retain(|case| facts.allows(case.value));
        if facts.covered_by(self.cases.len()) {
            self.default = None;
        }
    }
}
