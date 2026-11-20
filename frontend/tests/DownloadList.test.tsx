import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { DownloadList } from "@/components/settings/DownloadList";
import { formatBytes, relativeDay } from "@/lib/format";
import type { CatalogRow } from "@/lib/types";

const catalog: CatalogRow[] = [
  { tag: "gemma3:4b", sizeLabel: "3.3GB", sizeBytes: 3_300_000_000, note: "Google", thinkOk: true, ramHint: "16GB+", recommended: true },
  { tag: "deepseek-r1:1.5b", sizeLabel: "1.1GB", sizeBytes: 1_100_000_000, note: "⚠ 污染正文", thinkOk: false, recommended: false },
];

describe("DownloadList", () => {
  it("进行中的 tag 渲染进度条与取消；不推荐的折叠进「更多」", () => {
    const { container } = render(
      <DownloadList
        catalog={catalog}
        installedNames={[]}
        pulls={{
          "gemma3:4b": { tag: "gemma3:4b", status: "pulling", percent: 42, completed: 1, total: 3 },
        }}
        onDownload={vi.fn()}
        onCancel={vi.fn()}
      />
    );

    const bar = container.querySelector('[role="progressbar"]')!;
    expect(bar.getAttribute("aria-valuenow")).toBe("42");
    expect(screen.getByText("取消")).toBeTruthy();
    // 不推荐模型默认折叠：只在「更多」按钮里出现计数
    expect(screen.queryByText("deepseek-r1:1.5b")).toBeNull();
    expect(screen.getByText(/更多/)).toBeTruthy();
  });

  it("已装模型从可下载区剔除", () => {
    render(
      <DownloadList
        catalog={catalog}
        installedNames={["gemma3:4b"]}
        pulls={{}}
        onDownload={vi.fn()}
        onCancel={vi.fn()}
      />
    );
    expect(screen.queryByText("gemma3:4b")).toBeNull();
  });
});

describe("format", () => {
  it("formatBytes / relativeDay", () => {
    expect(formatBytes(800_000_000)).toBe("0.8GB");
    expect(formatBytes(3_300_000_000)).toBe("3.3GB");
    expect(formatBytes(0)).toBe("—");
    const now = new Date("2026-11-19T12:00:00Z");
    expect(relativeDay("2026-11-19T01:00:00Z", now)).toBe("今天");
    expect(relativeDay("2026-11-18T01:00:00Z", now)).toBe("昨天");
    expect(relativeDay("2026-11-14T01:00:00Z", now)).toBe("5 天前");
    expect(relativeDay("garbage", now)).toBe("");
  });
});
