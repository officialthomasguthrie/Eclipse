//! Inside the sandbox, before the command runs: Landlock rules that let it read and change only what
//! bwrap put there, and a seccomp filter that refuses the system calls a program in a sandbox has
//! no use for. Both hold for the command and for everything it starts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use landlock::{
    ABI, Access, AccessFs, RestrictionStatus, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetError, RulesetStatus, Scope, path_beneath_rules,
};
use seccompiler::{
    BackendError, BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition,
    SeccompFilter, SeccompRule, TargetArch,
};

/// The Landlock version penumbra needs all of, which Rift's kernel has: rights on files with
/// truncate and device ioctls, and the scopes that keep abstract sockets and signals inside the
/// sandbox.
const LANDLOCK: ABI = ABI::V6;

/// What a sandbox refuses with EPERM. Mounting and namespaces, the kernel's keyrings, BPF, perf,
/// userfaultfd, `io_uring` and file handles are ways into the kernel a command-line tool has no use
/// for. Modules, kexec, reboot, swap, setting the clock, the kernel log, accounting, quotas and I/O
/// ports need root, which a sandbox never has, and are refused all the same.
const REFUSED: &[libc::c_long] = &[
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_move_mount,
    libc::SYS_open_tree,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_userfaultfd,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_open_by_handle_at,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_settimeofday,
    libc::SYS_clock_settime,
    libc::SYS_syslog,
    libc::SYS_acct,
    libc::SYS_quotactl,
    libc::SYS_quotactl_fd,
    libc::SYS_iopl,
    libc::SYS_ioperm,
];

/// Terminal requests that type into the terminal the sandbox was started from, or take over its
/// console. ioctl refuses these and allows every other request.
const TERMINAL: &[u64] = &[libc::TIOCSTI, libc::TIOCLINUX];

/// From here on this process and what it starts can read what is under `read` and change what is
/// under `write`, and nothing else. A path that is not there is left out.
pub fn landlock(read: &[PathBuf], write: &[PathBuf]) -> Result<(), String> {
    let status = restrict(read, write)
        .map_err(|error| format!("Could not add the Landlock rules: {error}."))?;
    if status.ruleset == RulesetStatus::FullyEnforced {
        Ok(())
    } else {
        Err("This kernel does not enforce all of the Landlock rules a sandbox needs.".to_string())
    }
}

fn restrict(read: &[PathBuf], write: &[PathBuf]) -> Result<RestrictionStatus, RulesetError> {
    Ruleset::default()
        .handle_access(AccessFs::from_all(LANDLOCK))?
        .scope(Scope::from_all(LANDLOCK))?
        .create()?
        .add_rules(path_beneath_rules(read, AccessFs::from_read(LANDLOCK)))?
        .add_rules(path_beneath_rules(write, AccessFs::from_all(LANDLOCK)))?
        .restrict_self()
}

/// Adds the seccomp filter to this process and what it starts.
pub fn seccomp() -> Result<(), String> {
    let program =
        program().map_err(|error| format!("Could not make the seccomp filter: {error}."))?;
    seccompiler::apply_filter(&program)
        .map_err(|error| format!("Could not add the seccomp filter: {error}."))?;
    #[cfg(target_arch = "x86_64")]
    seccompiler::apply_filter(&x32())
        .map_err(|error| format!("Could not add the seccomp filter for x32: {error}."))?;
    Ok(())
}

/// The filter for this computer's architecture. A system call made as another architecture's, a
/// 32-bit one on x86-64, ends the process.
fn program() -> Result<BpfProgram, BackendError> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> =
        REFUSED.iter().map(|&number| (number, Vec::new())).collect();
    let terminal = TERMINAL
        .iter()
        .map(|&request| {
            SeccompRule::new(vec![SeccompCondition::new(
                1,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Eq,
                request,
            )?])
        })
        .collect::<Result<Vec<_>, _>>()?;
    rules.insert(libc::SYS_ioctl, terminal);
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM.unsigned_abs()),
        TargetArch::try_from(std::env::consts::ARCH)?,
    )?;
    BpfProgram::try_from(filter)
}

/// A second filter that refuses every system call in the x32 range with ENOSYS. The numbers above
/// are x86-64's, and the same calls made through the x32 table would not match them.
#[cfg(target_arch = "x86_64")]
fn x32() -> BpfProgram {
    use seccompiler::sock_filter;
    // BPF_LD | BPF_W | BPF_ABS: the system call's number, at the start of seccomp_data
    const LOAD_NUMBER: u16 = 0x20;
    // BPF_JMP | BPF_JGE | BPF_K and BPF_RET | BPF_K
    const JUMP_AT_LEAST: u16 = 0x35;
    const RETURN: u16 = 0x06;
    const X32: u32 = 0x4000_0000;
    vec![
        sock_filter {
            code: LOAD_NUMBER,
            jt: 0,
            jf: 0,
            k: 0,
        },
        sock_filter {
            code: JUMP_AT_LEAST,
            jt: 0,
            jf: 1,
            k: X32,
        },
        sock_filter {
            code: RETURN,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::ENOSYS.unsigned_abs(),
        },
        sock_filter {
            code: RETURN,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_has_every_refusal() {
        let program = program().unwrap();
        // the architecture check, then a comparison, two jumps, the refusal and the fall through for
        // each call, and more for ioctl's requests
        assert!(program.len() > 3 + REFUSED.len() * 5, "{}", program.len());
        for &number in REFUSED {
            let number = u32::try_from(number).unwrap();
            assert!(program.iter().any(|line| line.k == number), "{number}");
        }
    }
}
