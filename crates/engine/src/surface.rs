//! The command surface: every registered command id with the parameters it names, kept in
//! [`PATH`] so a removed or renamed command or parameter is a deliberate change in a pull request.
//! The command line, Python, the Assistant and MCP, saved scripts and plugins call commands by id
//! and by parameter key, and a change there breaks them without a compile error.
//!
//! The registry is checked against the file by `command_surface_matches_the_snapshot` (an engine
//! test, so it runs in `cargo xtask ci`); `cargo xtask commands` rewrites the file.

use crate::{CommandSpec, param_entries};

/// Where the snapshot is kept, from the repository's root.
pub const PATH: &str = "crates/engine/command-surface.txt";

/// The snapshot of `commands`: one line per command, sorted by id, `id key key? …`. A key ends in
/// `?` when it may be left out. The declared types are left out: they are prose (`power?: 2`,
/// `"name": the layer's name`), so rewording a description would change the file. Neither is the
/// help text. A command that takes no parameters, or describes them without keys, is its id alone.
pub fn text(commands: &[CommandSpec]) -> String {
    let mut lines: Vec<String> = commands.iter().map(line).collect();
    lines.sort();
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// One command's line of the snapshot: every key the description names, as `param_keys` reads
/// them, so `"lon" | "x": degrees` records both.
fn line(c: &CommandSpec) -> String {
    let mut line = c.id.to_string();
    param_entries(c.params, |key, optional| {
        line.push(' ');
        line.push_str(key);
        if optional {
            line.push('?');
        }
    });
    line
}

/// The differences between two snapshots, one message per command that was removed, added or
/// whose parameters changed (empty when they agree).
pub fn differences(saved: &str, current: &str) -> Vec<String> {
    let id = |line: &str| line.split(' ').next().unwrap_or("").to_string();
    let by_id = |text: &str| text.lines().map(|l| (id(l), l.to_string())).collect::<std::collections::BTreeMap<_, _>>();
    let (saved, current) = (by_id(saved), by_id(current));
    let mut out = Vec::new();
    for (id, line) in &saved {
        match current.get(id) {
            None => out.push(format!("removed: {line}")),
            Some(now) if now != line => out.push(format!("changed: was `{line}`, now `{now}`")),
            Some(_) => {}
        }
    }
    out.extend(current.iter().filter(|(id, _)| !saved.contains_key(*id)).map(|(_, line)| format!("added: {line}")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A removed, renamed or reshaped command or parameter fails here, so it is a decision in the
    /// pull request that makes it: the command line, Python, MCP, saved scripts and plugins call
    /// commands by id and key. An added command fails too, so the file never goes stale.
    #[test]
    fn command_surface_matches_the_snapshot() {
        let (saved, current) = (include_str!("../command-surface.txt"), text(&crate::cmd::all()));
        let diff = differences(saved, &current);
        // Equal ids and keys but another text: a repeated id, or lines out of order.
        assert!(
            saved == current,
            "the registered commands differ from {PATH}:\n{}\nIf that is intended, run `cargo xtask commands` and commit the file (a removal or \
             rename breaks scripts, the command line and plugins that call it)",
            if diff.is_empty() { "the same ids and keys, but a repeated id or another order".to_string() } else { diff.join("\n") }
        );
    }

    /// A key written as an alternative (`"lon" | "x": degrees`) is in the snapshot like any other.
    #[test]
    fn alternative_keys_are_recorded() {
        let spec =
            crate::CommandSpec::new("a.go", "Go", "", |_, _| Ok(Default::default())).params(r#"{"lon" | "x": degrees, "n"?: number}"#);
        assert_eq!(line(&spec), "a.go lon x n?");
    }

    /// The differences name what moved: a command gone, a key gone, a key made required and a command new.
    #[test]
    fn differences_name_each_change() {
        let saved = "a.gone x\na.keep x y?\na.keys x y\na.required x y?\n";
        let now = "a.keep x y?\na.keys x\na.new\na.required x y\n";
        assert_eq!(
            differences(saved, now),
            [
                "removed: a.gone x",
                "changed: was `a.keys x y`, now `a.keys x`",
                "changed: was `a.required x y?`, now `a.required x y`",
                "added: a.new"
            ]
        );
        assert!(differences(saved, saved).is_empty());
    }
}
