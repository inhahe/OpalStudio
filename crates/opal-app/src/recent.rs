//! Lists of what was used lately, the latest first: the scripts run, the entries typed into
//! each number field.

/// Puts `item` first in `list`, once (taking it out from further down), and keeps at most
/// `most` of them. Returns whether the list changed (not if `item` was first already).
pub fn latest_first<T: PartialEq>(list: &mut Vec<T>, item: T, most: usize) -> bool {
    if list.first() == Some(&item) {
        return false;
    }
    list.retain(|e| *e != item);
    list.insert(0, item);
    list.truncate(most);
    true
}

/// `items`, the latest first, as [`latest_first`] keeps them: each once, where it came
/// latest, and at most `most` of them.
pub fn latest_of<T: PartialEq>(items: impl DoubleEndedIterator<Item = T>, most: usize) -> Vec<T> {
    let mut list = Vec::new();
    for item in items.rev() {
        latest_first(&mut list, item, usize::MAX);
    }
    list.truncate(most);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_comes_first_and_once() {
        let mut list = vec!["b", "a"];
        assert!(latest_first(&mut list, "a", 3));
        assert_eq!(list, ["a", "b"]);
        assert!(!latest_first(&mut list, "a", 3), "first already, nothing changed");
        latest_first(&mut list, "c", 2);
        assert_eq!(list, ["c", "a"]);
        assert_eq!(latest_of(["x", "y", "x", "z"].into_iter(), 2), ["x", "y"]);
    }
}
