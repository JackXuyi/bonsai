import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { BonsaiFace } from "@/components/face/BonsaiFace";

describe("BonsaiFace", () => {
  it("props → data-stage / data-dormant 映射", () => {
    const { container } = render(
      <BonsaiFace state="calm" frame={0} stage={2} dormant={false} dayIndex={5} seed={9} />
    );
    const svg = container.querySelector("svg")!;
    expect(svg.getAttribute("data-stage")).toBe("2");
    expect(svg.getAttribute("data-dormant")).toBe("false");
  });

  it("S3 是叠在 S2 上的 overlay：s=3 时 S2/S3 分组同时点亮，dormant 时嘴拉平", () => {
    const { container } = render(
      <BonsaiFace state="calm" frame={0} stage={3} dormant={false} dayIndex={7} seed={1} />
    );
    const on = container.querySelectorAll("g.stage-g.on");
    const ds = Array.from(on).map((g) => g.getAttribute("data-s"));
    expect(ds).toEqual(["2", "3"]);

    const { container: c2 } = render(
      <BonsaiFace state="calm" frame={0} stage={2} dormant={true} dayIndex={0} seed={1} />
    );
    const mouth = c2.querySelector("path.mouth")!;
    expect(mouth.getAttribute("d")).toMatch(/^M92 \d+(\.\d+)? L108 \d+(\.\d+)?$/);
  });
});
