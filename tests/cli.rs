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

/// 子命令必须出现在帮助里：`brief` 是本变更对用户的唯一新入口。
#[test]
fn help_lists_brief_subcommand() {
    let output = binary().arg("--help").output().expect("启动成功");
    assert!(output.status.success(), "--help 应返回 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("brief"),
        "帮助应列出 brief 子命令：{stdout}"
    );
}

/// 清除三个 OpenCode 变量，避免开发者本机环境把测试变成另一组配置。
fn isolated() -> Command {
    let mut command = binary();
    command
        .env_remove("OPENCODE_API_KEY")
        .env_remove("OPENCODE_MODEL")
        .env_remove("OPENCODE_BASE_URL");
    command
}

/// `--json` 属于默认列表流程，与 `brief` 互斥；clap 应在任何网络请求前就以 2 退出。
#[test]
fn json_conflicts_with_brief() {
    let output = isolated()
        .args(["--json", "brief"])
        .output()
        .expect("启动成功");
    assert_eq!(output.status.code(), Some(2), "参数冲突应以 2 退出");
    assert!(output.stdout.is_empty(), "参数错误时标准输出应为空");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.is_empty(), "参数错误应输出到标准错误");
    assert!(
        stderr.contains("Usage") || stderr.contains("--help"),
        "标准错误应包含用法说明：{stderr}"
    );
}

/// `brief` 子命令不接受 `--json`：简报只有中文文本一种形式。
#[test]
fn brief_rejects_json_after_subcommand() {
    let output = isolated()
        .args(["brief", "--json"])
        .output()
        .expect("启动成功");
    assert_eq!(output.status.code(), Some(2), "参数错误应以 2 退出");
    assert!(output.stdout.is_empty(), "参数错误时标准输出应为空");
    assert!(
        !output.stderr.is_empty(),
        "参数错误应输出到标准错误：{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 三个配置场景共同的要求：在发起任何网络请求前以退出码 2 结束。
/// 测试把 `OPENCODE_BASE_URL` 指向不可能连通的地址；若配置校验晚于联网，
/// 就无法通过这条断言。
#[test]
fn brief_missing_api_key_exits_two_before_network() {
    let output = isolated()
        .env("OPENCODE_MODEL", "kimi-k3")
        .env("OPENCODE_BASE_URL", "http://127.0.0.1:1")
        .arg("brief")
        .output()
        .expect("启动成功");
    assert_eq!(output.status.code(), Some(2), "缺少 API Key 应以 2 退出");
    assert!(output.stdout.is_empty(), "配置错误时标准输出应为空");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OPENCODE_API_KEY"),
        "标准错误应指明缺少的变量：{stderr}"
    );
}

#[test]
fn brief_missing_model_exits_two_before_network() {
    let output = isolated()
        .env("OPENCODE_API_KEY", "oc_sk_secret_for_test")
        .env("OPENCODE_BASE_URL", "http://127.0.0.1:1")
        .arg("brief")
        .output()
        .expect("启动成功");
    assert_eq!(output.status.code(), Some(2), "缺少模型 ID 应以 2 退出");
    assert!(output.stdout.is_empty(), "配置错误时标准输出应为空");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OPENCODE_MODEL"),
        "标准错误应指明缺少的变量：{stderr}"
    );
    assert!(
        !stderr.contains("oc_sk_secret_for_test"),
        "标准错误不得回显 API Key：{stderr}"
    );
}

#[test]
fn brief_invalid_base_url_exits_two_before_network() {
    // 除“不是地址”外，重点覆盖复审指出的形态：`https:example.invalid` 会被
    // `Url::parse` 规范化成 `https://example.invalid/`，必须在解析前拒绝。
    // 反斜杠同理：HTTP(S) 是 special scheme，解析器把 `\` 当成分隔符 `/`，
    // `https://\example.invalid` 会变成 `https://example.invalid/`，且能绕过
    // “空 authority”检查；因此必须在原始字符串上拒绝。
    // `example.invalid` 是保留的无效域名，测试不会真的请求它。
    for base in [
        "not a url",
        "https:example.invalid",
        "http:example.invalid",
        "http:///inference",
        "http://exa mple.invalid",
        "https://\\example.invalid",
        "http://127.0.0.1:8080/pa\\th",
    ] {
        let output = isolated()
            .env("OPENCODE_API_KEY", "oc_sk_secret_for_test")
            .env("OPENCODE_MODEL", "kimi-k3")
            .env("OPENCODE_BASE_URL", base)
            .arg("brief")
            .output()
            .expect("启动成功");
        assert_eq!(
            output.status.code(),
            Some(2),
            "非法 Base URL {base:?} 应以 2 退出"
        );
        assert!(output.stdout.is_empty(), "配置错误时标准输出应为空");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("OPENCODE_BASE_URL"),
            "标准错误应指明出错的变量：{stderr}"
        );
        assert!(
            !stderr.contains("oc_sk_secret_for_test"),
            "标准错误不得回显 API Key：{stderr}"
        );
    }
}
