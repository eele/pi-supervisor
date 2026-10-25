//! I1 — runtime token push (Rust port of scripts/push-tokens.cmd).
//!
//! Two secrets, neither baked into the image:
//!   %USERPROFILE%\.ssh\agent-bridge-token → /home/agent/.ssh/agent-bridge-token
//!   %USERPROFILE%\.pi-hub\token           → /home/agent/.pi/agent/pi-hub-token
//!
//! Discipline (unchanged): tokens travel over the sbx exec STDIN only —
//! never argv, never logs; host file must be exactly 32 lowercase hex chars.

use crate::util;

struct TokenSpec {
    host_rel: &'static str, // relative to %USERPROFILE%
    dest: &'static str,
    hint: &'static str,
}

const SPECS: [TokenSpec; 2] = [
    TokenSpec {
        host_rel: ".ssh\\agent-bridge-token",
        dest: "/home/agent/.ssh/agent-bridge-token",
        hint: "this machine's pageant-tcp-bridge reads the same file",
    },
    TokenSpec {
        host_rel: ".pi-hub\\token",
        dest: "/home/agent/.pi/agent/pi-hub-token",
        hint: "created on first run of pi-hub/build-and-deploy.sh",
    },
];

/// Exactly 32 lowercase hex chars, first line wins (cmd for /f parity).
pub fn valid_token(line: &str) -> bool {
    let t = line.trim();
    t.len() == 32 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Push both tokens; failures are collected (one missing token must not
/// hide the other's result), mirroring the cmd script's FAIL accumulation.
pub fn push_all(sandbox: &str) -> Result<String, String> {
    let home = std::env::var("USERPROFILE").map_err(|_| "USERPROFILE is not set".to_string())?;
    let mut errors: Vec<String> = Vec::new();
    let mut ok_count = 0;

    for spec in SPECS {
        let host_path = format!("{home}\\{}", spec.host_rel);
        match push_one(sandbox, &host_path, spec.dest, spec.hint) {
            Ok(()) => ok_count += 1,
            Err(e) => errors.push(e),
        }
    }

    if errors.is_empty() {
        Ok(format!("push-tokens: all tokens installed in {sandbox}"))
    } else if ok_count > 0 {
        Err(format!(
            "push-tokens: partial — {} delivered, {} failed: {}",
            ok_count,
            errors.len(),
            errors.join("; ")
        ))
    } else {
        Err(format!("push-tokens: {}", errors.join("; ")))
    }
}

fn push_one(sandbox: &str, host_path: &str, dest: &str, hint: &str) -> Result<(), String> {
    let raw = std::fs::read_to_string(host_path)
        .map_err(|_| format!("host token not found: {host_path} (hint: {hint})"))?;
    let first_line = raw.lines().next().unwrap_or("");
    if !valid_token(first_line) {
        return Err(format!("{host_path} is not a 32-hex-char token"));
    }
    // Exactly the cmd's remote command: umask 077 + cat from stdin + chmod 600.
    let dir = dest.rsplit_once('/').map(|(d, _)| d).unwrap_or("/");
    let sh = format!(
        "umask 077 && mkdir -p {dir} && cat > {dest} && chmod 600 {dest} && echo installed {dest}"
    );
    util::sbx_exec_stdin(sandbox, &sh, first_line.trim().as_bytes(), 60_000)
        .map(|_| ())
        .map_err(|e| format!("failed to push {dest} into {sandbox}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_validation_real() {
        assert!(valid_token("0123456789abcdef0123456789abcdef"));
        assert!(!valid_token("0123456789abcdef0123456789abcde")); // 31
        assert!(!valid_token("0123456789abcdef0123456789abcdef0")); // 33
        assert!(!valid_token("0123456789ABCDEF0123456789abcdef")); // uppercase
        assert!(!valid_token("g123456789abcdef0123456789abcdef")); // non-hex
    }
}
