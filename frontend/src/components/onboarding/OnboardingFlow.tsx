"use client";

// onboarding（P1-3 边界问答）：全屏模态，走完才能进主界面。
// 年龄屏是硬门槛（未成年 → 终态页），其余全部可跳过。
// 问题文案由 Rust 的 onboarding_status 下发（single source of truth）。
import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { onboardingStatus, onboardingSubmit } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { toast, useUiStore } from "@/lib/stores/uiStore";
import type { OnboardingAnswer, OnboardingQuestion } from "@/lib/types";

type ObStep = "age" | "minor" | "crisis" | "question" | "done";

// Browser-preview fallback mirrors the Rust QUESTIONS (demo only).
const OB_QUESTIONS_FALLBACK: OnboardingQuestion[] = [
  { id: "q1", kind: "text", prompt: "有什么话题是我永远不该主动提的？", hint: "比如钱、家人、某段经历。多条用顿号隔开，也可以跳过。", options: [] },
  { id: "q2", kind: "text", prompt: "有什么话是我永远不该对你说的？", hint: "比如「你应该冷静」这种。多条用顿号隔开。", options: [] },
  { id: "q3", kind: "choice", prompt: "半夜可以打扰你吗？", hint: "主动搭话只有一天一次的额度，这里定它敢不敢用。", options: ["随便打扰", "只在工作时间", "永远别主动"] },
  { id: "q4", kind: "text", prompt: "我会把你记在本地数据库里。有哪类事是不准记的？", hint: "写下来的这类内容会停止进入记忆抽取。多条用顿号隔开。", options: [] },
  { id: "q5", kind: "choice", prompt: "我记性不好，猜错你别惯着我，直接说。什么语气你能接受？", hint: "这决定它纠正你、被纠正时的措辞基调。", options: ["直说就行", "委婉点", "别猜我"] },
];

export function OnboardingFlow() {
  const router = useRouter();
  const setToolResult = useUiStore((s) => s.setToolResult);
  const windowShownAt = useUiStore((s) => s.windowShownAt);

  const [visible, setVisible] = useState(false);
  const [step, setStep] = useState<ObStep>("age");
  const [questions, setQuestions] = useState<OnboardingQuestion[]>([]);
  const [qIndex, setQIndex] = useState(0);
  const collectedRef = useRef<OnboardingAnswer[]>([]);
  const [ageGroup, setAgeGroup] = useState<string | null>(null);
  const [choice, setChoice] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [doneText, setDoneText] = useState("");

  const check = useCallback(async () => {
    if (!inTauri) return;
    try {
      const st = await onboardingStatus();
      if (st.done && st.age_group !== "minor") {
        setVisible(false);
        return;
      }
      setQuestions(st.questions?.length ? st.questions : OB_QUESTIONS_FALLBACK);
      setVisible(true);
      if (st.done && st.age_group === "minor") {
        setStep("minor"); // permanent gate
      } else {
        setAgeGroup(null);
        setQIndex(0);
        collectedRef.current = [];
        setStep("age");
      }
    } catch {
      /* status unreachable: don't block the UI */
    }
  }, []);

  // boot + tray「重新做一次引导」（window:shown 断点）
  useEffect(() => {
    if (inTauri) check();
  }, [check, windowShownAt]);

  // 浏览器预览：走一遍流程便于检视
  useEffect(() => {
    if (!inTauri) {
      setQuestions(OB_QUESTIONS_FALLBACK);
      setVisible(true);
      setStep("age");
    }
  }, []);

  const q = questions[qIndex];

  function pickAge(g: string) {
    setAgeGroup(g);
    if (g === "minor") {
      setStep("minor");
      return;
    }
    setStep("crisis");
  }

  function ackCrisis() {
    setQIndex(0);
    renderQuestionReset();
    setStep("question");
  }

  function renderQuestionReset() {
    setChoice(null);
    setText("");
  }

  function collectCurrent(): boolean {
    if (!q) return false;
    if (q.kind === "choice") {
      if (!choice) return false; // nothing selected yet
      collectedRef.current.push({ question: q.id, answer: choice, skipped: false });
      return true;
    }
    const answer = (text || "").trim();
    collectedRef.current.push({ question: q.id, answer, skipped: !answer });
    return true;
  }

  function skip() {
    if (!q) return;
    collectedRef.current.push({
      question: q.id,
      answer: q.kind === "choice" ? "" : (text || "").trim(),
      skipped: true,
    });
    advance();
  }

  function next() {
    if (!collectCurrent()) {
      toast("选一个，或者跳过");
      return;
    }
    advance();
  }

  function advance() {
    const nextIdx = qIndex + 1;
    if (nextIdx >= questions.length) {
      const c = collectedRef.current;
      const n = c.filter((a) => !a.skipped && ["q1", "q2", "q4"].includes(a.question)).length;
      const m = c.filter((a) => !a.skipped && ["q3", "q5"].includes(a.question)).length;
      setDoneText(
        n || m
          ? `记住了 ${n} 条底线、${m} 条偏好。猜错的时候，直接骂我就行。`
          : "什么都没写也没关系——想说的时候，随时告诉我。"
      );
      setStep("done");
    } else {
      setQIndex(nextIdx);
      renderQuestionReset();
      setStep("question");
    }
  }

  async function finish() {
    setVisible(false);
    if (!inTauri) {
      toast("（预览）引导流程走完了");
      return;
    }
    try {
      const r = await onboardingSubmit({
        ageGroup,
        crisisAck: ageGroup === "adult",
        answers: collectedRef.current,
      });
      if (r.minor) {
        // Terminal gate: show the minor page and stay there.
        setStep("minor");
        setVisible(true);
        return;
      }
      toast(`阿盆记住了 ${r.boundaries_added} 条底线、${r.preferences_added} 条偏好`);
      router.push("/");
    } catch (err) {
      setToolResult({ title: "引导提交失败", text: String(err) });
    }
  }

  if (!visible) return null;

  return (
    <section className="onboarding" aria-label="初次引导">
      <header className="ob-head">
        <p className="ob-name">阿盆</p>
        <p className="ob-sub">先说清楚几件事，我们再开始。</p>
      </header>

      {step === "age" && (
        <div className="ob-step">
          <p className="ob-title">开始之前，确认一件事</p>
          <p className="ob-text">
            你成年了吗？按《人工智能拟人化互动服务管理暂行办法》，未成年人暂时用不了这个应用。
          </p>
          <div className="ob-actions">
            <button className="ob-btn" onClick={() => pickAge("adult")}>
              我成年了
            </button>
            <button className="ob-btn" onClick={() => pickAge("minor")}>
              我未成年
            </button>
          </div>
        </div>
      )}

      {step === "minor" && (
        <div className="ob-step">
          <p className="ob-title">未成年人暂不可用</p>
          <p className="ob-text">这一步没有开关。把它关掉就好，盆栽不会怪你。</p>
          <div className="ob-actions">
            <button className="ob-btn">{/* terminal: nowhere to go */}知道了</button>
          </div>
        </div>
      )}

      {step === "crisis" && (
        <div className="ob-step">
          <p className="ob-title">一件事，必须先说</p>
          <p className="ob-text crisis-card">
            我不是心理医生，也接不住所有的话。如果你哪天说到伤害自己的念头，我会立刻收起玩笑，把这两个号码递给你：
            <br />
            <strong>北京心理危机研究与干预中心 010-82951332（24 小时）</strong>
            <br />
            <strong>全国心理援助热线 12356</strong>
          </p>
          <div className="ob-actions">
            <button className="ob-btn" onClick={ackCrisis}>
              我记住了
            </button>
          </div>
        </div>
      )}

      {step === "question" && q && (
        <div className="ob-step">
          <p className="ob-title">{q.prompt}</p>
          <p className="ob-text ob-hint">{q.hint || ""}</p>
          {q.kind === "choice" ? (
            <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginTop: 10 }}>
              {q.options.map((opt) => (
                <button
                  key={opt}
                  className={`ob-btn ob-choice${choice === opt ? " active" : ""}`}
                  onClick={() => setChoice(opt)}
                >
                  {opt}
                </button>
              ))}
            </div>
          ) : (
            <textarea
              className="ob-input"
              placeholder="想到什么写什么，或直接跳过"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
          )}
          <div className="ob-actions">
            <button className="ob-btn ob-quiet" onClick={skip}>
              跳过
            </button>
            <button className="ob-btn" onClick={next}>
              {qIndex === questions.length - 1 ? "就这些" : "下一条"}
            </button>
          </div>
        </div>
      )}

      {step === "done" && (
        <div className="ob-step">
          <p className="ob-title">行，都记下了。</p>
          <p className="ob-text">{doneText}</p>
          <p className="ob-text ob-hint">底线随时可以在「工具 · 边界清单」里增删。</p>
          <div className="ob-actions">
            <button className="ob-btn" onClick={finish}>
              开始吧
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
