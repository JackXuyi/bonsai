"use client";

// 云端 opt-in 警示（spec-m2 §4.1）：每会话一次，三条必须说到——
// 文本上传 / 人格与记忆不出机器 / key 明文存在本机。
interface CloudOptinModalProps {
  onCancel: () => void;
  onConfirm: () => void;
}

export function CloudOptinModal({ onCancel, onConfirm }: CloudOptinModalProps) {
  return (
    <div className="mconfirm" role="dialog" aria-modal="true" aria-label="切换到云端模型">
      <div className="mconfirm-box">
        <p className="mconfirm-title">切换到云端模型？</p>
        <p className="mconfirm-sub">
          之后每一轮对话和记忆抽取的文本都会发送到该服务商，不再只留在本机。
        </p>
        <ul className="optin-list">
          <li>人格卡与记忆库不出机器</li>
          <li>危机拦截、年龄门禁照常生效</li>
          <li>API Key 明文存在本机数据库</li>
        </ul>
        <div className="mconfirm-actions">
          <button type="button" className="btn-ghost" onClick={onCancel}>
            还是本地
          </button>
          <button type="button" className="btn" onClick={onConfirm}>
            我知道了，切
          </button>
        </div>
      </div>
    </div>
  );
}
