import { describe, expect, it } from "vitest";
import { pickReplyText } from "@/hooks/useChat";

describe("pickReplyText（plan-retry §7.2 取长）", () => {
  it("本地已有流式正文时，兜底句不得覆盖", () => {
    const local = "说到一半的一百个字".repeat(2);
    const fallback =
      "（盆栽这会儿状态不太好，脑子转不动——稍后再聊，别走开。）";
    expect(pickReplyText(local, fallback)).toBe(local);
  });

  it("本地更长则取本地", () => {
    expect(pickReplyText("已经流出很多字了啊", "短")).toBe("已经流出很多字了啊");
  });

  it("outcome 更长则取 outcome", () => {
    expect(pickReplyText("短", "这是完整答复，比本地长")).toBe(
      "这是完整答复，比本地长"
    );
  });
});
