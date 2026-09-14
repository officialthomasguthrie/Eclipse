//! The network switch in nftables. Penumbra's table `inet penumbra` holds a set of the cgroups of
//! the sandboxes whose app has its network off, and drops every packet that leaves from or arrives
//! at a socket in one of them, loopback included. A sandbox's cgroup is its scope in the owner's
//! user manager, five levels below the root of the cgroup tree. nft reads a cgroup's path when it
//! is added, so only scopes that run go into the set, and the set is made again on every change.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::{fs, io};

use libeclipse::penumbra::{app_of_scope, name_problem};

/// How deep the scope of a sandbox is: user.slice, the account's slice, its user manager,
/// app.slice, the scope.
pub const LEVEL: u32 = 5;

/// The scope of one sandbox that runs now.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scope {
    /// Its cgroup, from the root of the cgroup tree, without a slash in front.
    pub path: String,
    /// The app it is a sandbox of.
    pub app: String,
    /// The account whose user manager it is in.
    pub uid: u32,
}

/// The scope of every sandbox in the cgroup tree at `root`, sorted by path.
pub fn scopes(root: &Path) -> io::Result<Vec<Scope>> {
    let mut found = Vec::new();
    let users = match fs::read_dir(root.join("user.slice")) {
        Ok(users) => users,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for user in users.flatten() {
        let slice = user.file_name();
        let Some((slice, uid)) = slice
            .to_str()
            .and_then(|slice| Some((slice, uid_of_slice(slice)?)))
        else {
            continue;
        };
        let apps = format!("user.slice/{slice}/user@{uid}.service/app.slice");
        // a user manager that is not running, or that stopped while this looked
        let Ok(units) = fs::read_dir(root.join(&apps)) else {
            continue;
        };
        for unit in units.flatten() {
            let unit = unit.file_name();
            if let Some((unit, app)) = unit
                .to_str()
                .and_then(|unit| Some((unit, app_of_scope(unit)?)))
            {
                found.push(Scope {
                    path: format!("{apps}/{unit}"),
                    app: app.to_string(),
                    uid,
                });
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The account of a slice named `user-<uid>.slice`.
fn uid_of_slice(slice: &str) -> Option<u32> {
    let digits = slice.strip_prefix("user-")?.strip_suffix(".slice")?;
    // the number has to give the same name back, so the path is the one on disk
    digits
        .parse::<u32>()
        .ok()
        .filter(|uid| uid.to_string() == digits)
}

/// The scope of a sandbox a process runs in, from its `/proc/<pid>/cgroup`, when the scope is in
/// the user manager of `uid`.
pub fn scope_of(cgroup: &str, uid: u32) -> Option<Scope> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::/"))?;
    let unit = path.strip_prefix(&format!(
        "user.slice/user-{uid}.slice/user@{uid}.service/app.slice/"
    ))?;
    let app = app_of_scope(unit)?;
    Some(Scope {
        path: path.to_string(),
        app: app.to_string(),
        uid,
    })
}

/// The apps in the file that keeps which apps have their network off, one name a line.
pub fn read_off(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|name| name_problem(name).is_none())
        .map(ToString::to_string)
        .collect()
}

/// The text of that file.
pub fn write_off(off: &BTreeSet<String>) -> String {
    off.iter().fold(String::new(), |mut text, name| {
        let _ = writeln!(text, "{name}");
        text
    })
}

/// nft's script that makes the table again from nothing, with `cut` in its set. Replacing a table
/// in one script is one transaction, so there is no moment without it.
pub fn table(cut: &[&str]) -> String {
    let mut script = String::from(
        "destroy table inet penumbra\ntable inet penumbra {\n\tset off {\n\t\ttype cgroupsv2\n\t}\n",
    );
    for hook in ["output", "input"] {
        let _ = writeln!(
            script,
            "\tchain {hook} {{\n\t\ttype filter hook {hook} priority filter; policy accept;\n\
             \t\tsocket cgroupv2 level {LEVEL} @off drop\n\t}}"
        );
    }
    script.push_str("}\n");
    script.push_str(&elements(cut));
    script
}

/// nft's script that leaves exactly `cut` in the set.
pub fn elements(cut: &[&str]) -> String {
    let mut script = String::from("flush set inet penumbra off\n");
    if !cut.is_empty() {
        let quoted: Vec<String> = cut.iter().map(|path| format!("\"{path}\"")).collect();
        let _ = writeln!(
            script,
            "add element inet penumbra off {{ {} }}",
            quoted.join(", ")
        );
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_are_found_in_every_user_manager() {
        let root = std::env::temp_dir().join(format!("penumbra-scopes-{}", std::process::id()));
        let manager = "user.slice/user-1000.slice/user@1000.service";
        for folder in [
            &format!("{manager}/app.slice/app-penumbra-fetcher-12.scope"),
            &format!("{manager}/app.slice/app-penumbra-yt-dlp-40.scope"),
            &format!("{manager}/app.slice/app-org.gnome.Nautilus-3.scope"),
            &format!("{manager}/session.slice/app-penumbra-hidden-5.scope"),
            "user.slice/user-1000.slice/session-2.scope",
            "user.slice/user-1001.slice/user@1001.service/app.slice/app-penumbra-fetcher-7.scope",
            "user.slice/user-01001.slice/user@1001.service/app.slice/app-penumbra-odd-8.scope",
            "user.slice/user-1002.slice",
        ] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let found = scopes(&root);
        fs::remove_dir_all(&root).unwrap();
        let scope = |path: &str, app: &str, uid| Scope {
            path: path.to_string(),
            app: app.to_string(),
            uid,
        };
        assert_eq!(
            found.unwrap(),
            vec![
                scope(
                    "user.slice/user-1000.slice/user@1000.service/app.slice/app-penumbra-fetcher-12.scope",
                    "fetcher",
                    1000
                ),
                scope(
                    "user.slice/user-1000.slice/user@1000.service/app.slice/app-penumbra-yt-dlp-40.scope",
                    "yt-dlp",
                    1000
                ),
                scope(
                    "user.slice/user-1001.slice/user@1001.service/app.slice/app-penumbra-fetcher-7.scope",
                    "fetcher",
                    1001
                ),
            ]
        );
        assert_eq!(scopes(&root).unwrap(), vec![]);
    }

    #[test]
    fn a_process_is_in_a_scope_of_its_own_account() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-penumbra-fetcher-12.scope\n";
        assert_eq!(
            scope_of(cgroup, 1000),
            Some(Scope {
                path: "user.slice/user-1000.slice/user@1000.service/app.slice/app-penumbra-fetcher-12.scope"
                    .to_string(),
                app: "fetcher".to_string(),
                uid: 1000,
            })
        );
        assert_eq!(scope_of(cgroup, 1001), None);
        for cgroup in [
            "0::/user.slice/user-1000.slice/session-2.scope\n",
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-penumbra-fetcher-12.scope/inner\n",
            "0::/system.slice/aura.service\n",
            "",
        ] {
            assert_eq!(scope_of(cgroup, 1000), None, "{cgroup}");
        }
    }

    #[test]
    fn the_file_holds_one_app_a_line() {
        let off = read_off("fetcher\n\ncurl\nno/such\n  yt-dlp  \n");
        assert_eq!(
            off.iter().map(String::as_str).collect::<Vec<_>>(),
            ["curl", "fetcher", "yt-dlp"]
        );
        assert_eq!(write_off(&off), "curl\nfetcher\nyt-dlp\n");
        assert_eq!(write_off(&BTreeSet::new()), "");
    }

    #[test]
    fn the_table_drops_both_ways_for_the_set() {
        let path =
            "user.slice/user-1000.slice/user@1000.service/app.slice/app-penumbra-fetcher-12.scope";
        assert_eq!(
            table(&[path]),
            format!(
                "destroy table inet penumbra\n\
                 table inet penumbra {{\n\
                 \tset off {{\n\t\ttype cgroupsv2\n\t}}\n\
                 \tchain output {{\n\t\ttype filter hook output priority filter; policy accept;\n\
                 \t\tsocket cgroupv2 level 5 @off drop\n\t}}\n\
                 \tchain input {{\n\t\ttype filter hook input priority filter; policy accept;\n\
                 \t\tsocket cgroupv2 level 5 @off drop\n\t}}\n\
                 }}\n\
                 flush set inet penumbra off\n\
                 add element inet penumbra off {{ \"{path}\" }}\n"
            )
        );
        assert_eq!(elements(&[]), "flush set inet penumbra off\n");
        assert_eq!(
            elements(&["a.scope", "b.scope"]),
            "flush set inet penumbra off\nadd element inet penumbra off { \"a.scope\", \"b.scope\" }\n"
        );
    }
}
