"""Exercise the packaged executable over plugin protocol 4 against an offline host fixture."""
import hashlib
import json
import os
import select
import struct
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

package = Path(sys.argv[1]).resolve()
# 与 backend/Cargo.toml 固定的宿主 SDK 提交一致（codex-proxy-rs v3.29.6）
PROTOCOL_VERSION = 4

with tempfile.TemporaryDirectory(prefix="model-health-smoke-") as directory:
    root = Path(directory).resolve()
    with tarfile.open(package) as archive:
        for member in archive.getmembers():
            path = root / member.name
            assert member.isfile() and path.resolve().is_relative_to(root)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(archive.extractfile(member).read())
            path.chmod(0o755 if member.mode & 0o111 else 0o644)
    manifest = json.loads((root / "plugin.json").read_text())
    for path, digest in manifest["package"]["files"].items():
        assert hashlib.sha256((root / path).read_bytes()).hexdigest() == digest, path
    assert [s["namespace"] for s in manifest["state"]] == ["runs", "candy"], "legacy runs namespace must stay declared"
    # 打包 CLI 不校验二进制架构；这里核对 ELF e_machine 与包目标一致，并在同架构容器中运行。
    target = manifest["package"]["target"]
    assert target["os"] == "linux", target
    platform, machine = {"x86_64": ("linux/amd64", 0x3E), "aarch64": ("linux/arm64", 0xB7)}[target["architecture"]]
    binary = (root / "bin/plugin").read_bytes()
    assert binary[:4] == b"\x7fELF" and struct.unpack_from("<H", binary, 18)[0] == machine, target
    html = (root / "web/index.html").read_text()
    assert '<script defer src="./app.js"></script>' in html and 'type="module"' not in html
    javascript = (root / "web/app.js").read_text()
    assert "preview-request-" not in javascript and "demo-k" not in javascript
    # 按宿主 PreparedPackage / ProcessSpec 合同准备：私有可写包目录、非 root、空环境。
    # 只读根文件系统与 /tmp:noexec 保持实际部署条件，不能为了测试放开 /tmp。
    root.chmod(0o755)
    container = f"model-health-smoke-{os.getpid()}"
    command = ["docker", "run", "--rm", "--name", container, "-i", "--platform", platform,
               "--network", "none", "--read-only", "--user", "10001:10001",
               "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=64m",
               "--tmpfs", "/package:rw,exec,nosuid,nodev,size=256m,uid=10001,gid=10001,mode=0700",
               "-v", f"{root}:/artifact:ro", "-w", "/package", "--entrypoint", "/bin/sh",
               os.environ.get("SMOKE_IMAGE", "debian:bookworm-slim"),
               "-ec", 'test "$(id -u)" = 10001; grep -q " /tmp .*noexec" /proc/mounts; '
               'cp -R /artifact/. /package/; chmod -R go-rwx /package; chmod 700 bin/plugin; '
               'exec /usr/bin/env -i ./bin/plugin']
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)

    # SDK 协议 4：业务 JSON（握手配置、调用参数、结果）不进控制元数据，
    # 以 data_bytes 声明长度放在载荷前缀；Call 的入口快照另有 request_context_bytes 前缀。
    def data_slot(message):
        kind = message["type"]
        if kind == "hello":
            return message["handshake"], "configuration"
        if kind in ("call", "callback"):
            return message, "params"
        if kind == "result":
            return message, "result"
        return None, None

    def send(message, payload=b""):
        message = json.loads(json.dumps(message))
        owner, field = data_slot(message)
        data = b""
        if owner is not None:
            data = json.dumps(owner.pop(field), separators=(",", ":")).encode()
            message["data_bytes"] = len(data)
        body = json.dumps(message, separators=(",", ":")).encode()
        process.stdin.write(struct.pack(">IQ", len(body), len(data) + len(payload)) + body + data + payload)
        process.stdin.flush()

    def read_exact(length):
        result = b""
        while len(result) < length:
            if not select.select([process.stdout], [], [], 40)[0]:
                raise TimeoutError("packaged plugin did not respond")
            chunk = process.stdout.read(length - len(result))
            if not chunk:
                raise RuntimeError("packaged plugin exited: " + process.stderr.read().decode())
            result += chunk
        return result

    def receive():
        meta, total = struct.unpack(">IQ", read_exact(12))
        assert 0 < meta <= 65536 and total < 2 * 1024 * 1024
        message = json.loads(read_exact(meta))
        request_bytes = message.pop("request_context_bytes", 0)
        data_bytes = message.pop("data_bytes", 0)
        assert request_bytes == 0, "plugin frames never carry an ingress request snapshot"
        owner, field = data_slot(message)
        assert (owner is not None) == (data_bytes > 0), message
        if owner is not None:
            owner[field] = json.loads(read_exact(data_bytes))
        return message, read_exact(total - data_bytes)

    host = {"state": {}, "callbacks": [], "bodies": [], "answer": "最少需要 21 个"}

    def runtime_account():
        return {"account_id": "smoke-account", "provider_id": "openai", "credential_revision": 1,
                "name": "Offline fixture", "email": "  smoke@example.com  ", "upstream_user_id": "fixture-user", "upstream_account_id": "fixture-workspace",
                "plan_type": "pro", "authentication_kind": "oauth", "enabled": True, "credential_state": "ready",
                "has_refresh_token": True, "access_token_expires_at_ms": None, "next_refresh_at_ms": None}

    def callback(method, params, payload):
        host["callbacks"].append(method)
        if method.startswith("host.state."):
            assert params["namespace"] == "candy", "legacy runs namespace is never touched"
        if method == "host.auth.list":
            assert json.loads(payload)["provider_id"] == "openai"
            return {}, json.dumps({"accounts": [runtime_account()], "next_cursor": None}).encode()
        if method == "host.auth.get_runtime":
            assert json.loads(payload)["account_id"] == "smoke-account"
            return {}, json.dumps(runtime_account()).encode()
        if method == "host.keys.list":
            return {"keys": [{"id": "smoke-key", "name": "Offline key", "enabled": True}], "next_cursor": None}, b""
        if method == "host.models.list":
            assert params["client_key_id"] == "smoke-key" and params["protocol"] == "openai"
            return {"models": ["gpt-6-astra"]}, b""
        if method == "host.state.get":
            return {"record": host["state"].get(params["key"])}, b""
        if method == "host.state.put":
            previous = host["state"].get(params["key"])
            assert params["expected_version"] == (previous["version"] if previous else None)
            assert len(params["value"]["records"]) <= 20
            version = previous["version"] + 1 if previous else 1
            host["state"][params["key"]] = {"value": params["value"], "version": version, "schema_version": 1}
            return {"version": version}, b""
        if method == "host.state.delete":
            assert params["expected_version"] == host["state"][params["key"]]["version"]
            del host["state"][params["key"]]
            return {"deleted": True}, b""
        if method == "host.model.execute_stream":
            assert params["account_id"] == "smoke-account" and params["client_key_id"] == "smoke-key"
            assert params["provider"] == "openai" and params["protocol"] == "openai" and params["model"] == "gpt-6-astra"
            assert host["state"]["smoke-account"]["value"]["records"][-1]["status"] == "pending", "pending is persisted before sending"
            body = json.loads(payload)
            assert body["stream"] is True and body["store"] is False and "tools" not in body
            assert "苹果味" in body["input"][0]["content"]
            host["bodies"].append(body)
            return {"request_id": f'fixture-request-{len(host["bodies"])}', "stream": "fixture-stream"}, b""
        if method == "host.model.stream_read":
            data = {"type": "response.completed", "response": {"id": "fixture-response", "status": "completed",
                    "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": host["answer"]}]}],
                    "usage": {"input_tokens": 310, "output_tokens": 40, "output_tokens_details": {"reasoning_tokens": 12}}}}
            metadata = json.dumps({"wire": {"protocol": "openai", "payload": {"kind": "json", "event": None, "data": data}}}).encode()
            event = b"GPE2" + struct.pack(">III", len(metadata), 0, 0) + metadata
            return {"events": 1, "end": True}, b"HME1" + struct.pack(">II", 1, len(event)) + event
        raise AssertionError("unexpected callback: " + method)

    def api(path, body=None):
        context["call_id"] += 2
        identifier = context["call_id"]
        send({"type": "call", "id": identifier, "method": "management.handle", "context": context,
              "params": {"method": "GET" if body is None else "POST", "path": path, "query": "",
                         "content_type": None if body is None else "application/json", "headers": []}},
             b"" if body is None else json.dumps(body).encode())
        while True:
            message, payload = receive()
            if message["type"] == "result":
                assert message["id"] == identifier
                return message["result"]["status"], json.loads(payload)
            assert message["type"] == "callback", message
            assert message["parent_id"] == identifier
            result, response = callback(message["method"], message["params"], payload)
            send({"type": "result", "id": message["id"], "result": result}, response)

    def run(effort):
        return api("api/candy/run", {"account_id": "smoke-account", "key_id": "smoke-key", "model": "gpt-6-astra", "effort": effort})

    try:
        assert manifest["package"]["protocolVersion"] == PROTOCOL_VERSION, manifest["package"]
        send({"type": "hello", "handshake": {"protocol_version": PROTOCOL_VERSION, "artifact_sha256": hashlib.sha256(package.read_bytes()).hexdigest(), "plugin_id": "local.model-health", "instance_id": "smoke", "generation": 1, "incarnation": "smoke", "configuration": {}, "contributes": manifest["contributes"]}})
        ready, _ = receive()
        assert ready["type"] == "ready" and ready["protocol_version"] == PROTOCOL_VERSION, ready
        context = {"call_id": 1, "instance_id": "smoke", "generation": 1, "incarnation": "smoke", "stage": "management", "timeout_ms": 120000, "resource_stream": False, "resource_scope_id": "smoke", "request_id": None, "attempt_id": None, "account_id": None, "credential_revision": None}
        status, boot = api("api/bootstrap")
        assert status == 200, boot
        assert boot["answer"] == "21" and "五角星形" in boot["prompt"] and boot["efforts"][0] == "none"
        assert boot["accounts"][0]["id"] == "smoke-account" and boot["accounts"][0]["records"] == []
        assert boot["accounts"][0]["email"] == "  smoke@example.com  " and boot["keys"][0]["id"] == "smoke-key"
        assert host["callbacks"] == ["host.auth.list", "host.keys.list", "host.state.get"]
        status, error = api("api/models", {})
        assert status == 400 and error["error"]["message"] == "请求参数格式错误", error
        status, models = api("api/models", {"key_id": "smoke-key"})
        assert status == 200 and models["models"] == ["gpt-6-astra"]

        status, correct = run("low")
        assert status == 200 and correct["record"]["status"] == "correct", correct
        assert host["bodies"][-1]["reasoning"] == {"effort": "low"}
        assert correct["record"]["reasoning_tokens"] == 12 and "answer" not in correct["record"]
        host["answer"] = "答案是 121 个"
        status, wrong = run("none")
        assert status == 200 and wrong["record"]["status"] == "wrong", wrong
        assert "reasoning" not in host["bodies"][-1]
        assert [r["status"] for r in wrong["records"]] == ["correct", "wrong"]
        status, detail = api("api/candy/record", {"account_id": "smoke-account", "record_id": wrong["record"]["id"]})
        assert status == 200 and detail["record"]["answer"] == "答案是 121 个" and detail["record"]["request_id"] == "fixture-request-2"
        status, boot = api("api/bootstrap")
        assert status == 200 and boot["accounts"][0]["records"] == wrong["records"]
        status, cleared = api("api/candy/clear", {"account_ids": ["smoke-account"]})
        assert status == 200 and cleared == {"cleared": 1, "skipped": []} and host["state"] == {}
        print("CANDY PASS: correct=PASS; wrong=PASS; effort_none=PASS; pending_before_send=PASS; detail=PASS; clear=PASS; pinned_account_key=PASS; legacy_runs_untouched=PASS; real_model_calls=0", flush=True)

        send({"type": "shutdown"})
        process.wait(timeout=15)
        assert process.returncode == 0, process.stderr.read().decode()
        print(f"PACKAGE PASS: hashes=PASS; tmp_noexec=PASS; nonroot=PASS; empty_env=PASS; readonly_root=PASS; protocol={PROTOCOL_VERSION}; version={manifest['version']}; accounts=1; keys=1; error_http=400; real_model_calls=0")
    finally:
        if process.poll() is None:
            subprocess.run(["docker", "rm", "-f", container], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
            process.wait(timeout=15)
