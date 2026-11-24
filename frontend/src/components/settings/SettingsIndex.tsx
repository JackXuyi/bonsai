"use client";

// 设置索引：分组行 → 分区详情（plan-chat-first-ui v2）。
import Link from "next/link";
import { SecondaryChrome } from "@/components/layout/SecondaryChrome";
import { settingsGroups, settingsHref } from "@/lib/settingsSections";

export function SettingsIndex() {
  const groups = settingsGroups();

  return (
    <>
      <SecondaryChrome title="设置" />
      <div className="tab-body settings-index">
        {groups.map((g) => (
          <section key={g.group} className="settings-group">
            <h2 className="settings-group-label">{g.group}</h2>
            <ul className="settings-index-list">
              {g.items.map((item) => (
                <li key={item.id}>
                  <Link
                    href={settingsHref(item.id)}
                    className="settings-index-row"
                  >
                    <span>{item.label}</span>
                    <span className="settings-index-chevron" aria-hidden="true">
                      ›
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </>
  );
}
