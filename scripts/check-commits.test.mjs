import { describe, expect, it } from "vitest";
import { MAX_SUBJECT, messageProblems, subjectProblems } from "./check-commits.mjs";

describe("subjectProblems", () => {
  it.each([
    "fix(gateway): keep tool call ids",
    "docs(agents-md): add commit message rules",
    "chore(release): 0.4.4",
    "feat(claude-code): support `--model` aliases",
  ])("accepts %s", (s) => {
    expect(subjectProblems(s)).toEqual([]);
  });

  it("rejects a subject without a scope", () => {
    expect(subjectProblems("fix: keep ids")).toEqual(["not in the form `type(scope): description`"]);
  });

  it("rejects free-form and git's default revert subjects", () => {
    expect(subjectProblems("Fix gateway auth")).toHaveLength(1);
    expect(subjectProblems('Revert "fix(gateway): keep ids"')).toHaveLength(1);
  });

  it("rejects an unknown type", () => {
    expect(subjectProblems("feature(gateway): add x")[0]).toMatch(/type `feature`/);
  });

  it("rejects upper-case, underscored and empty scopes", () => {
    expect(subjectProblems("fix(Gateway): x")[0]).toMatch(/scope/);
    expect(subjectProblems("fix(claude_code): x")[0]).toMatch(/scope/);
    expect(subjectProblems("fix(): x")[0]).toMatch(/scope/);
    expect(subjectProblems("fix(-a): x")[0]).toMatch(/scope/);
  });

  it("rejects a capitalised description and a trailing period", () => {
    expect(subjectProblems("fix(gateway): Keep ids")).toEqual(["description starts with an upper-case letter"]);
    expect(subjectProblems("fix(gateway): keep ids.")).toEqual(["description ends with a period"]);
  });

  it("rejects an empty description and a double space", () => {
    expect(subjectProblems("fix(gateway): ")).toEqual(["description is empty"]);
    expect(subjectProblems("fix(gateway):  keep ids")).toEqual(["extra space before the description"]);
  });

  it("counts characters, not bytes, against the length limit", () => {
    const prefix = "fix(gateway): ";
    expect(subjectProblems(prefix + "a".repeat(MAX_SUBJECT - prefix.length))).toEqual([]);
    expect(subjectProblems(prefix + "a".repeat(MAX_SUBJECT - prefix.length + 1))).toEqual([
      `longer than ${MAX_SUBJECT} characters`,
    ]);
    expect(subjectProblems(prefix + "é".repeat(MAX_SUBJECT - prefix.length))).toEqual([]);
  });
});

describe("messageProblems", () => {
  it("accepts a subject with a body and human co-authors", () => {
    expect(
      messageProblems("fix(gateway): keep ids\r\n\r\nWhy it matters.\r\n\r\nCo-authored-by: Jane <jane@example.com>\r\n"),
    ).toEqual([]);
  });

  it("requires a blank line after the subject", () => {
    expect(messageProblems("fix(gateway): keep ids\nbody")).toEqual(["no blank line between subject and body"]);
  });

  it.each([
    "Co-Authored-By: Claude <noreply@anthropic.com>",
    "co-authored-by: Copilot <copilot@github.com>",
    "🤖 Generated with [Claude Code](https://claude.com/claude-code)",
  ])("flags AI attribution: %s", (trailer) => {
    expect(messageProblems(`fix(gateway): keep ids\n\n${trailer}`)).toEqual([
      "body has AI attribution (Co-Authored-By / Generated with)",
    ]);
  });

  it("does not look for attribution in the subject", () => {
    expect(messageProblems("docs(readme): mention claude code")).toEqual([]);
  });
});
