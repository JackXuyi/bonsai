import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
  boundaryDelete,
  calendarSetConfig,
  dialogueChatStream,
  facePreview,
  growthAck,
  loopAutorestartSet,
  loopsStatus,
  onboardingSubmit,
  perceptionSetConfig,
} from "@/lib/commands";

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
});

// commands smoke：每个封装调用了正确命令名与参数（抽样）
describe("commands", () => {
  it("封装透传正确的命令名与 snake_case 参数", async () => {
    await facePreview("greeted");
    expect(invokeMock).toHaveBeenCalledWith("face_preview", { state: "greeted" });

    await growthAck(12);
    expect(invokeMock).toHaveBeenCalledWith("growth_ack", { id: 12 });

    await boundaryDelete(3);
    expect(invokeMock).toHaveBeenCalledWith("boundary_delete", { id: 3 });

    await calendarSetConfig({ enabled: true });
    expect(invokeMock).toHaveBeenCalledWith("calendar_set_config", { enabled: true });

    await perceptionSetConfig({ enabled: true, clipboard: false, active_app: true, downloads: false });
    expect(invokeMock).toHaveBeenCalledWith("perception_set_config", {
      cfg: { enabled: true, clipboard: false, active_app: true, downloads: false },
    });

    const channel = { toJSON: () => 0 };
    await dialogueChatStream("hi", "default", channel);
    expect(invokeMock).toHaveBeenCalledWith("dialogue_chat_stream", {
      userText: "hi",
      sessionId: "default",
      onDelta: channel,
    });

    await onboardingSubmit({ ageGroup: "adult", crisisAck: true, answers: [] });
    expect(invokeMock).toHaveBeenCalledWith("onboarding_submit", {
      payload: { ageGroup: "adult", crisisAck: true, answers: [] },
    });

    await loopsStatus();
    expect(invokeMock).toHaveBeenCalledWith("loops_status", undefined);

    await loopAutorestartSet(true);
    expect(invokeMock).toHaveBeenCalledWith("loop_autorestart_set", {
      enabled: true,
    });
  });
});
