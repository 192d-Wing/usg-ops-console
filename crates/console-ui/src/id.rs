//! Unique DOM id generation for label/description wiring.

use std::cell::Cell;
use yew::prelude::*;

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// Returns a process-unique id such as `cui-field-3`. `prefix` should be a
/// short static identifier (letters, digits, `-`).
pub fn next_id(prefix: &str) -> AttrValue {
    let n = NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    });
    AttrValue::from(format!("cui-{prefix}-{n}"))
}

/// Hook returning an id that is stable for the lifetime of the component.
#[hook]
pub fn use_id(prefix: &'static str) -> AttrValue {
    (*use_memo((), move |_| next_id(prefix))).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_prefixed() {
        let ids: HashSet<String> = (0..1000).map(|_| next_id("x").to_string()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(ids.iter().all(|i| i.starts_with("cui-x-")));
    }
}
