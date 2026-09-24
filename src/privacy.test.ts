import { describe, expect, it } from "vitest";
import { maskHost, scrub, scrubAlways, scrubHost, scrubVar, setPrivacy } from "./privacy";

describe("scrubAlways", () => {
  it("hides remote hosts and ports, keeps paths and local addresses", () => {
    expect(scrubAlways("gpt-6-sol · http://64.83.33.80:8080/responses")).toBe("gpt-6-sol · http://•••/responses");
    expect(scrubAlways("https://api.example.com/v1")).toBe("https://•••/v1");
    expect(scrubAlways("https://user:pw@relay.example.com:443/v1")).toBe("https://•••/v1");
    expect(scrubAlways("http://127.0.0.1:18650/relay/v1")).toBe("http://127.0.0.1:18650/relay/v1");
    expect(scrubAlways("http://localhost:1420")).toBe("http://localhost:1420");
    expect(scrubAlways("连接 64.83.33.80:8080 失败")).toBe("连接 ••• 失败");
    expect(scrubAlways("版本 1.2.3")).toBe("版本 1.2.3");
  });

  it("hides the account folder in paths", () => {
    expect(scrubAlways("C:\\Users\\klpz2\\.codex\\config.toml")).toBe("C:\\Users\\•••\\.codex\\config.toml");
    expect(scrubAlways("C:/Users/klpz2/.claude")).toBe("C:/Users/•••/.claude");
    expect(scrubAlways("/home/klpz/.codex/auth.json")).toBe("/home/•••/.codex/auth.json");
    expect(scrubAlways("\\\\wsl$\\Ubuntu\\home\\klpz\\.codex")).toBe("\\\\wsl$\\Ubuntu\\home\\•••\\.codex");
    expect(scrubAlways("~/.codex/config.toml")).toBe("~/.codex/config.toml");
  });

  it("hides keys and key hints", () => {
    expect(scrubAlways('api_key = "sk-ant-api03-abcdef123456"')).toBe('api_key = "•••"');
    expect(scrubAlways("Authorization: Bearer abc.def-123456789")).toBe("Authorization: Bearer •••");
    expect(scrubAlways("••••1234")).toBe("••••");
    expect(scrubAlways("token a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6")).toBe("token •••");
    // Words and ordinary ids stay.
    expect(scrubAlways("model gpt-5.5 · claude-sonnet-5")).toBe("model gpt-5.5 · claude-sonnet-5");
    expect(scrubAlways("sk-1")).toBe("sk-1");
  });

  it("maskHost keeps this machine", () => {
    expect(maskHost("127.0.0.1:18650")).toBe("127.0.0.1:18650");
    expect(maskHost("relay.example.com:8443")).toBe("•••");
  });
});

describe("scrub", () => {
  it("only masks while privacy mode is on", () => {
    expect(scrub("https://a.example.com")).toBe("https://a.example.com");
    setPrivacy(true);
    expect(scrub("https://a.example.com")).toBe("https://•••");
    expect(scrub(null)).toBeNull();
    setPrivacy(false);
    expect(scrub("https://a.example.com")).toBe("https://a.example.com");
  });

  it("scrubHost masks schemeless hosts, not plain names", () => {
    setPrivacy(true);
    expect(scrubHost("relay.example.com/v1")).toBe("•••/v1");
    expect(scrubHost("64.83.33.80:8080")).toBe("•••");
    expect(scrubHost("127.0.0.1:18650/relay/v1")).toBe("127.0.0.1:18650/relay/v1");
    expect(scrubHost("OpenRouter")).toBe("OpenRouter");
    setPrivacy(false);
    expect(scrubHost("relay.example.com/v1")).toBe("relay.example.com/v1");
  });

  it("scrubVar masks placeholders by what they hold", () => {
    setPrivacy(true);
    expect(scrubVar("machine", "KLPZ-PC")).toBe("•••");
    expect(scrubVar("station", "api.example.com")).toBe("•••");
    expect(scrubVar("station", "OpenRouter")).toBe("OpenRouter");
    expect(scrubVar("url", "https://api.example.com/v1")).toBe("https://•••/v1");
    expect(scrubVar("name", "我的中转")).toBe("我的中转");
    setPrivacy(false);
    expect(scrubVar("machine", "KLPZ-PC")).toBe("KLPZ-PC");
  });
});
