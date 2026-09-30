import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { AgentId, AgentState } from "../api";
import { t } from "../i18n";
import { VENDORS } from "../templates";
import type { DropdownOption } from "./Dropdown";
import { ProviderDialog } from "./ProviderDialog";

// Expose the closed dropdown's choices while rendering the actual dialog's conditions.
vi.mock("./Dropdown", () => ({
  Dropdown: ({ options }: { options: DropdownOption[] }) => <select>{options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}</select>,
}));
// The desktop language store has no server snapshot; keep its translations and read its
// current language without subscribing while rendering this test's static markup.
vi.mock("../i18n", async (importOriginal) => {
  const original = await importOriginal<typeof import("../i18n")>();
  return { ...original, useLang: original.getLang };
});

const render = (id: AgentId) => renderToStaticMarkup(<ProviderDialog
  st={{ id, name: id, settings: [], providers: [], catalog: null } as unknown as AgentState}
  draft={{}} editing={null} gatewayRoute={undefined} gateway={null} ensureGateway={vi.fn()} onCreateCatalog={vi.fn()} onCommitCatalog={vi.fn()}
  onSave={vi.fn()} onClose={vi.fn()} flash={vi.fn()} library={{ groups: [], onAdd: vi.fn() }}
/>);

describe("ProviderDialog library choices", () => {
  it("offers the provider library to Gemini without incompatible vendor templates", () => {
    const html = render("gemini");
    expect(html).toContain('value="@library"');
    expect(html).toContain(t("templatePicker.library"));
    for (const vendor of VENDORS) expect(html).not.toContain(`value="${vendor.id}"`);
  });

  it("keeps vendor templates and the library available to gateway-capable agents", () => {
    const html = render("codex");
    expect(html).toContain('value="@library"');
    for (const vendor of VENDORS) expect(html).toContain(`value="${vendor.id}"`);
  });
});
