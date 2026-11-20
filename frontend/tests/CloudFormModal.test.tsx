import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CloudFormModal } from "@/components/settings/CloudFormModal";

function fill(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}

describe("CloudFormModal", () => {
  it("OpenAI 官方：隐藏端点输入，保存时强制官方地址", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const onClose = vi.fn();
    render(<CloudFormModal onClose={onClose} onSave={onSave} onTest={vi.fn()} />);

    fireEvent.change(screen.getByLabelText("类型"), { target: { value: "openai" } });
    expect(screen.queryByLabelText("接口地址（/v1）")).toBeNull();
    expect(screen.getByText(/官方端点固定为/)).toBeTruthy();

    fill("名称", "官方");
    fill("API Key", "sk-xyz");
    fill("模型名", "gpt-4o-mini");
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(onSave.mock.calls[0][0]).toMatchObject({
      kind: "openai",
      baseUrl: "https://api.openai.com/v1",
      model: "gpt-4o-mini",
    });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("Claude 官方（spec-m2.1）：选项可选，端点锁到 anthropic", async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    render(<CloudFormModal onClose={vi.fn()} onSave={onSave} onTest={vi.fn()} />);

    const option = screen.getByRole("option", { name: "Claude 官方" }) as HTMLOptionElement;
    expect(option.disabled).toBe(false);

    fireEvent.change(screen.getByLabelText("类型"), { target: { value: "claude" } });
    expect(screen.queryByLabelText("接口地址（/v1）")).toBeNull();
    expect(screen.getByText(/官方端点固定为 https:\/\/api\.anthropic\.com/)).toBeTruthy();

    fill("名称", "Claude");
    fill("API Key", "sk-ant-xyz");
    fill("模型名", "claude-3-5-sonnet-latest");
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(onSave.mock.calls[0][0]).toMatchObject({
      kind: "claude",
      baseUrl: "https://api.anthropic.com",
      model: "claude-3-5-sonnet-latest",
    });
  });

  it("空名称不发请求，直接报错", () => {
    const onSave = vi.fn();
    render(<CloudFormModal onClose={vi.fn()} onSave={onSave} onTest={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText("名字不能空")).toBeTruthy();
  });

  it("测试连接：成功后内联显示延迟，失败提示重查三项", async () => {
    const onTest = vi.fn().mockResolvedValueOnce({ latencyMs: 430 });
    render(<CloudFormModal onClose={vi.fn()} onSave={vi.fn()} onTest={onTest} />);

    fill("名称", "DeepSeek");
    fill("接口地址（/v1）", "https://api.deepseek.com/v1");
    fill("API Key", "sk-abcdefgh");
    fill("模型名", "deepseek-chat");
    fireEvent.click(screen.getByRole("button", { name: "测试连接" }));

    await waitFor(() => expect(screen.getByText(/测试通过/)).toBeTruthy());
    expect(screen.getByText(/0\.4s/)).toBeTruthy();
    expect(onTest.mock.calls[0][0]).toMatchObject({
      kind: "custom",
      baseUrl: "https://api.deepseek.com/v1",
    });

    // 第二次失败：同一按钮走 fail 分支
    onTest.mockResolvedValueOnce(null);
    fireEvent.click(screen.getByRole("button", { name: "测试连接" }));
    await waitFor(() => expect(screen.getByText(/没通过/)).toBeTruthy());
  });
});
