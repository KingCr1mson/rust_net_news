//! 命令行参数层的测试：不联网，只验证 clap 的行为。

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rust_net_news"))
}

#[test]
fn unknown_argument_exits_nonzero_with_usage_on_stderr() {
    let output = binary().arg("--unknown-flag").output().expect("启动成功");
    assert!(!output.status.success(), "参数错误应返回非零退出码");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage") || stderr.contains("--help"),
        "标准错误应包含用法说明：{stderr}"
    );
    assert!(output.stdout.is_empty(), "参数错误时标准输出应为空");
}

#[test]
fn help_lists_json_flag() {
    let output = binary().arg("--help").output().expect("启动成功");
    assert!(output.status.success(), "--help 应返回 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--json"), "帮助应包含 --json：{stdout}");
}
