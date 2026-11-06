// @vitest-environment jsdom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentState, EnvInfo } from "./api";
import { setLang } from "./i18n";
import App from "./App";

const mock = vi.hoisted(() => ({
  listAgents: vi.fn(), listEnvs: vi.fn(), setEnv: vi.fn(), apply: vi.fn(),
  page: null as ComponentProps<typeof import("./components/AgentPage").AgentPage> | null,
}));

vi.mock("./api", async (load) => {
  const original = await load<typeof import("./api")>();
  return { ...original, api: { ...original.api,
    listAgents: mock.listAgents, listEnvs: mock.listEnvs, setEnv: mock.setEnv, apply: mock.apply,
    libraryList: vi.fn().mockResolvedValue([]), projectsList: vi.fn().mockResolvedValue([]),
    gatewayStatus: vi.fn().mockResolvedValue(null), preview: vi.fn().mockResolvedValue([]),
  } };
});
vi.mock("./prefs", async (load) => {
  const original = await load<typeof import("./prefs")>();
  return { ...original, applyPrefs: vi.fn(), loadPrefs: () => ({ ...original.loadPrefs(), autoLatency: false }) };
});
vi.mock("./attribution/bank", async (load) => ({ ...await load<typeof import("./attribution/bank")>(), loadBank: () => new Promise(() => {}) }));
vi.mock("./components/AgentPage", () => ({
  EDIT_MODEL_EVENT: "edit-model",
  AgentPage: (props: NonNullable<typeof mock.page>) => {
    mock.page = props;
    return <div data-testid="agent">{props.st.name}<button data-testid="edit" onClick={() => props.setDraft({
      setting: { op: "set_setting", key: "example", value: true },
    })}>Edit</button></div>;
  },
}));
vi.mock("./components/Aside", () => ({ Aside: ({ pending, busy, onApply }: { pending: number; busy: boolean; onApply: () => void }) => (
  <button data-testid="apply" disabled={!pending || busy} onClick={onApply}>Apply</button>
) }));
vi.mock("./components/Sidebar", () => ({ Sidebar: () => null }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const state = (name: string): AgentState => ({
  id: "claude", name, installed: true, version: null, running: false, mode: "single",
  configDir: "test", files: [], currentProvider: null, providers: [], catalog: null,
  catalogFile: null, settings: [], current: [], currentModel: null, notes: [], readonly: false,
  fixedPending: false, fixedPrompt: false, restartable: false,
});
const envs = (id: string): EnvInfo[] => ["windows", "wsl:Ubuntu"].map((e) => ({ id: e, label: e, detail: "", current: e === id }));

describe("environment switching", () => {
  let host: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.clearAllMocks();
    await setLang("en");
    mock.listAgents.mockResolvedValue([state("Windows config")]);
    mock.listEnvs.mockResolvedValue(envs("windows"));
    mock.setEnv.mockImplementation(async (id: string) => { mock.listEnvs.mockResolvedValue(envs(id)); });
    mock.apply.mockResolvedValue({ state: state("WSL config"), files: [], backupDir: null });
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
    await act(async () => root.render(<App />));
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    host.remove();
    vi.unstubAllGlobals();
  });

  const click = async (selector: string) => {
    const button = host.querySelector<HTMLButtonElement>(selector);
    expect(button).not.toBeNull();
    await act(async () => button!.click());
  };
  const switchEnv = async () => {
    await click(".env-btn");
    await click('.env-item[aria-checked="false"]');
  };

  it("removes old editors while loading and binds new writes to the selected environment", async () => {
    const next = deferred<AgentState[]>();
    const oldPage = mock.page!;
    mock.listAgents.mockReturnValue(next.promise);
    await switchEnv();
    expect(host.querySelector('[data-testid="agent"]')).toBeNull();
    expect(host.querySelector<HTMLButtonElement>(".env-btn")!.disabled).toBe(true);
    const reads = mock.listAgents.mock.calls.length;
    await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "F5", bubbles: true })));
    expect(mock.listAgents).toHaveBeenCalledTimes(reads);
    await act(async () => oldPage.setDraft({ stale: { op: "set_setting", key: "old", value: true } }));
    await act(async () => next.resolve([state("WSL config")]));
    expect(host.textContent).toContain("WSL config");
    expect(host.querySelector<HTMLButtonElement>('[data-testid="apply"]')!.disabled).toBe(true);
    await click('[data-testid="edit"]');
    await click('[data-testid="apply"]');
    expect(mock.apply).toHaveBeenCalledWith("claude", [{ op: "set_setting", key: "example", value: true }], "wsl:Ubuntu");
  });

  it("does not restore old editors after a failed load or a late refresh", async () => {
    const oldRead = deferred<AgentState[]>();
    mock.listAgents.mockReturnValueOnce(oldRead.promise);
    await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "F5", bubbles: true })));
    const next = deferred<AgentState[]>();
    mock.listAgents.mockReturnValueOnce(next.promise);
    await switchEnv();
    await act(async () => next.reject(new Error("Failed to read WSL configs")));
    await act(async () => oldRead.resolve([state("Stale Windows config")]));
    expect(host.querySelector('[data-testid="agent"]')).toBeNull();
    expect(host.textContent).toContain("Failed to read WSL configs");
    expect(mock.apply).not.toHaveBeenCalled();
    mock.listAgents.mockResolvedValue([state("WSL config")]);
    await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "F5", bubbles: true })));
    expect(host.textContent).toContain("WSL config");
  });

  it("prevents switching while a write is still using the current environment", async () => {
    const write = deferred<{ state: AgentState; files: string[]; backupDir: null }>();
    mock.apply.mockReturnValue(write.promise);
    await click('[data-testid="edit"]');
    await click('[data-testid="apply"]');
    expect(mock.apply.mock.calls[0][2]).toBe("windows");
    expect(host.querySelector<HTMLButtonElement>(".env-btn")!.disabled).toBe(true);
    await click(".env-btn");
    expect(mock.setEnv).not.toHaveBeenCalled();
    await act(async () => write.resolve({ state: state("Windows config"), files: [], backupDir: null }));
    expect(host.querySelector<HTMLButtonElement>(".env-btn")!.disabled).toBe(false);
  });
});
