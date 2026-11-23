import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { SkillPreviewDialog } from "@/components/agent/SkillPreviewDialog";
import { SkillAddDialog } from "@/components/agent/SkillAddDialog";
import type { SkillDetail } from "@/lib/types";

const detail: SkillDetail = {
  id: "gmp-qa",
  name: "GMP 助手",
  description: "药品 GMP 规范问答",
  body: "你是 GMP 领域的资深 QA。",
  refs: [{ name: "条款.md", content: "第一条：保持洁净。" }],
  enabled: true,
  sizeBytes: 42,
};

const readMock = vi.fn().mockResolvedValue<SkillDetail>(detail);
const importMock = vi.fn().mockResolvedValue("gmp-qa");
const createMock = vi.fn().mockResolvedValue(undefined);
const remoteMock = vi.fn().mockResolvedValue("gmp-qa");
const pickMock = vi.fn().mockResolvedValue("/tmp/gmp-qa");
const onChanged = vi.fn();
const onClose = vi.fn();

vi.mock("@/lib/commands", async (importOriginal) => {
  const orig = await importOriginal<typeof import("@/lib/commands")>();
  return {
    ...orig,
    skillRead: (...args: unknown[]) => readMock(...args),
    skillImport: (...args: unknown[]) => importMock(...args),
    skillCreate: (...args: unknown[]) => createMock(...args),
    skillLoadRemote: (...args: unknown[]) => remoteMock(...args),
    fsPickDirectory: (...args: unknown[]) => pickMock(...args),
  };
});

beforeEach(() => {
  readMock.mockClear().mockResolvedValue(detail);
  importMock.mockClear().mockResolvedValue("gmp-qa");
  createMock.mockClear().mockResolvedValue(undefined);
  remoteMock.mockClear().mockResolvedValue("gmp-qa");
  pickMock.mockClear().mockResolvedValue("/tmp/gmp-qa");
  onChanged.mockClear();
  onClose.mockClear();
});

describe("SkillPreviewDialog", () => {
  it("loads the pack and renders body plus references", async () => {
    render(<SkillPreviewDialog open id="gmp-qa" onClose={onClose} />);
    await waitFor(() => expect(readMock).toHaveBeenCalledWith("gmp-qa"));
    await waitFor(() => expect(screen.getByTestId("skill-preview-body")).toBeTruthy());
    expect(screen.getByText(/已启用/)).toBeTruthy();
    expect(screen.getByText(/资深 QA/)).toBeTruthy();
    expect(screen.getByText("引用：条款.md")).toBeTruthy();
    expect(screen.getByText(/保持洁净/)).toBeTruthy();
  });

  it("surfaces a humanized read error", async () => {
    readMock.mockRejectedValue("skill_invalid");
    render(<SkillPreviewDialog open id="gone" onClose={onClose} />);
    await waitFor(() =>
      expect(screen.getByText(/技能包格式不对/)).toBeTruthy()
    );
  });
});

describe("SkillAddDialog", () => {
  it("import flow: pick directory then import with the picked path", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("选择目录"));
    await waitFor(() => expect(pickMock).toHaveBeenCalled());
    const input = screen.getByTestId("skill-import-source") as HTMLInputElement;
    await waitFor(() => expect(input.value).toBe("/tmp/gmp-qa"));
    fireEvent.click(screen.getByText("导入"));
    await waitFor(() => expect(importMock).toHaveBeenCalledWith("/tmp/gmp-qa"));
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("import flow: empty source is refused client-side", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("导入"));
    await waitFor(() =>
      expect(screen.getByText(/先用「选择目录」/)).toBeTruthy()
    );
    expect(importMock).not.toHaveBeenCalled();
  });

  it("import failure shows a humanized error and stays open", async () => {
    importMock.mockRejectedValue("skill_exists: skill id already exists");
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("选择目录"));
    await waitFor(() => expect(importMock).not.toHaveBeenCalled());
    fireEvent.click(screen.getByText("导入"));
    await waitFor(() => expect(importMock).toHaveBeenCalledWith("/tmp/gmp-qa"));
    await waitFor(() =>
      expect(screen.getByText(/同名技能包已存在/)).toBeTruthy()
    );
    expect(onClose).not.toHaveBeenCalled();
  });

  it("create flow: fills fields and calls skill_create", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("手写创建"));
    fireEvent.change(screen.getByPlaceholderText("my-skill"), {
      target: { value: "gmp-qa" },
    });
    fireEvent.change(screen.getByPlaceholderText("GMP 助手"), {
      target: { value: "GMP 助手" },
    });
    fireEvent.change(screen.getByPlaceholderText(/一句话说清/), {
      target: { value: "药品 GMP 规范问答" },
    });
    fireEvent.change(screen.getByTestId("skill-create-body"), {
      target: { value: "正文内容" },
    });
    fireEvent.click(screen.getByText("创建"));
    await waitFor(() =>
      expect(createMock).toHaveBeenCalledWith({
        id: "gmp-qa",
        name: "GMP 助手",
        description: "药品 GMP 规范问答",
        body: "正文内容",
      })
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("create flow: missing fields are refused client-side", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("手写创建"));
    fireEvent.click(screen.getByText("创建"));
    await waitFor(() =>
      expect(screen.getByText(/四项都要填/)).toBeTruthy()
    );
    expect(createMock).not.toHaveBeenCalled();
  });

  it("remote flow: load from a GitHub URL with a derived id", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("从远端加载"));
    fireEvent.change(screen.getByTestId("skill-remote-url"), {
      target: { value: "https://github.com/acme/skills/tree/main/gmp" },
    });
    fireEvent.click(screen.getByTestId("skill-remote-submit"));
    await waitFor(() =>
      expect(remoteMock).toHaveBeenCalledWith(
        "https://github.com/acme/skills/tree/main/gmp",
        undefined
      )
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("remote flow: explicit id override is passed through", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("从远端加载"));
    fireEvent.change(screen.getByTestId("skill-remote-url"), {
      target: { value: "https://example.com/packs/gmp/SKILL.md" },
    });
    fireEvent.change(screen.getByTestId("skill-remote-id"), {
      target: { value: "my-gmp" },
    });
    fireEvent.click(screen.getByTestId("skill-remote-submit"));
    await waitFor(() =>
      expect(remoteMock).toHaveBeenCalledWith(
        "https://example.com/packs/gmp/SKILL.md",
        "my-gmp"
      )
    );
  });

  it("remote flow: empty URL and network errors are surfaced", async () => {
    render(<SkillAddDialog open onClose={onClose} onChanged={onChanged} />);
    fireEvent.click(screen.getByText("从远端加载"));
    fireEvent.click(screen.getByTestId("skill-remote-submit"));
    await waitFor(() => expect(screen.getByText("先贴一个链接")).toBeTruthy());
    expect(remoteMock).not.toHaveBeenCalled();

    remoteMock.mockRejectedValue("skill_not_found");
    fireEvent.change(screen.getByTestId("skill-remote-url"), {
      target: { value: "https://github.com/acme/skills/tree/main/nope" },
    });
    fireEvent.click(screen.getByTestId("skill-remote-submit"));
    await waitFor(() => expect(screen.getByText(/没找到 SKILL.md/)).toBeTruthy());
    expect(onClose).not.toHaveBeenCalled();
  });
});
