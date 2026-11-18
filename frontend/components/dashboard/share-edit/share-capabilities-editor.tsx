"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { previewShareCapabilities } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  capabilityAmount,
  SHARE_OPERATIONS,
  OPERATION_LABELS,
  COVERAGE_LABELS,
  emptyCapabilityPolicy,
  type ShareCapabilityPolicy,
  type CapabilityConsent,
  type CapabilityPreview,
} from "@/lib/share-capabilities";
import type { ShareUserGrantMap } from "@/lib/types";

export function ShareCapabilitiesEditor({
  shareId,
  revision,
  policy,
  savedPolicy,
  grants,
  tokenLimit,
  disabled,
  onChange,
  onGrantOperationsChange,
  previewCapabilities = previewShareCapabilities,
}: {
  previewCapabilities?: typeof previewShareCapabilities;
  shareId: string;
  revision: number;
  policy?: ShareCapabilityPolicy;
  savedPolicy?: ShareCapabilityPolicy;
  grants: ShareUserGrantMap;
  tokenLimit: number;
  onGrantOperationsChange: (
    email: string,
    operations?: ShareCapabilityPolicy["allowedOperations"],
  ) => void;
  disabled?: boolean;
  onChange: (
    policy: ShareCapabilityPolicy,
    consent?: CapabilityConsent,
  ) => void;
}) {
  const { locale } = useLocaleText();
  const lang = locale.startsWith("zh") ? 0 : 1;
  const [preview, setPreview] = React.useState<CapabilityPreview>();
  const [error, setError] = React.useState("");
  const [accepted, setAccepted] = React.useState<string[]>([]);
  const [refresh, setRefresh] = React.useState(0);
  const [bulkEmails, setBulkEmails] = React.useState<string[]>([]);
  const [bulkOps, setBulkOps] = React.useState<
    ShareCapabilityPolicy["allowedOperations"]
  >([]);
  const selected = policy?.allowedOperations ?? [];
  const inheritedGrants = Object.values(grants).filter(
    (grant) => grant.active && grant.policy.allowedOperations === undefined,
  ).length;
  const requestKey = JSON.stringify([
    shareId,
    revision,
    selected,
    policy?.operationBindings,
    Object.entries(grants)
      .filter(([, g]) => g.active)
      .map(([email, g]) => [email, g.policy]),
    tokenLimit,
    refresh,
  ]);
  React.useEffect(() => {
    const abort = new AbortController();
    setPreview(undefined);
    setAccepted([]);
    setError("");
    const input = {
      expectedConfigRevision: revision,
      replacePolicies: true,
      policy: policy,
      policies: Object.fromEntries(
        Object.entries(grants)
          .filter(([, g]) => g.active)
          .map(([email, g]) => [email, g.policy]),
      ),
      shareTokenLimit: tokenLimit,
    };
    const timer = setTimeout(() => {
      void previewCapabilities(shareId, input, abort.signal)
        .then((value) => {
          if (!abort.signal.aborted) setPreview(value);
        })
        .catch((err) => {
          if (!abort.signal.aborted)
            setError(err instanceof Error ? err.message : String(err));
        });
    }, 200);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
    // The request key deliberately excludes server-owned audit fields / credentials.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestKey]);
  const choose = (
    operation: (typeof SHARE_OPERATIONS)[number],
    checked: boolean,
  ) => {
    const bindings = { ...policy?.operationBindings };
    delete bindings[operation];
    onChange({
      ...emptyCapabilityPolicy(),
      operationBindings: bindings,
      allowedOperations: checked
        ? [...selected, operation]
        : selected.filter((op) => op !== operation),
    });
  };
  const required =
    preview?.requiredAcknowledgements.filter(
      (ack) =>
        !savedPolicy?.coverageAcknowledgements?.some(
          (old) => old.condition === ack.condition,
        ),
    ) ?? [];
  const expired = !!preview && preview.expiresAt <= Date.now();
  return (
    <section
      className="grid gap-3 rounded-xl border border-slate-200 p-4"
      aria-label={lang === 0 ? "扩展能力" : "Extended capabilities"}
    >
      <div>
        <h3 className="font-medium">
          {lang === 0 ? "扩展能力" : "Extended capabilities"}
        </h3>
        <p className="mt-1 text-xs text-muted-foreground">
          {lang === 0
            ? "适用于 Token、美元和无限额调用。图片、文档、音频与视频理解输入继续可用。美元为 API 等值额度。"
            : "Applies to Token, USD and unlimited usage. Image, document, audio and video understanding inputs remain available. USD means API-equivalent usage."}
        </p>
      </div>
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? `${inheritedGrants} 个有效授权继承 Share 能力，保存后的权限变化将对这些授权生效。`
          : `${inheritedGrants} active grants inherit Share capabilities and will follow the saved permissions.`}
      </p>
      {!policy && preview?.policy.allowedOperations.length ? (
        <button
          type="button"
          className="text-left text-xs underline"
          disabled={disabled}
          onClick={() => onChange(preview.policy)}
        >
          {lang === 0
            ? "载入旧 Grok 明确授权的迁移草稿"
            : "Load migration draft for explicit legacy Grok permissions"}
        </button>
      ) : null}
      {!savedPolicy ? (
        <p className="text-xs text-amber-700">
          {lang === 0
            ? "旧授权策略：尚未独立控制。选择并确认后采用新策略；未勾选操作将关闭。"
            : "Legacy authorization: independent control is not active. Confirming adopts the new policy and disables unchecked operations."}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-sm text-red-600">
          {error}
        </p>
      ) : null}
      {!preview && !error ? (
        <p role="status">
          {lang === 0
            ? "正在读取 Server 能力…"
            : "Loading Server capabilities…"}
        </p>
      ) : null}
      {preview && !preview.ready ? (
        <p role="alert" className="text-xs text-amber-700">
          {lang === 0
            ? "需要 Router / Server 支持新协议并启用权威存储，当前不能保存新策略。"
            : "Router / Server protocol and authoritative storage must be ready before saving."}
        </p>
      ) : null}
      <div className="grid gap-3">
        {SHARE_OPERATIONS.map((operation) => {
          const rows =
            preview?.rows.filter((row) => row.operation === operation) ?? [];
          const available = rows.some((row) => row.controllable);
          return (
            <div key={operation} className="border-t border-slate-100 pt-2">
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={selected.includes(operation)}
                  disabled={
                    disabled ||
                    !preview?.ready ||
                    (!available && !selected.includes(operation))
                  }
                  onChange={(e) => choose(operation, e.target.checked)}
                />
                <span>{OPERATION_LABELS[operation][lang]}</span>
              </label>
              <div className="ml-6 text-xs text-muted-foreground">
                {rows.length
                  ? rows.map((row) => (
                      <p key={row.app}>
                        {selected.includes(operation) ? (
                          <input
                            aria-label={`${operation} ${row.app}`}
                            type="checkbox"
                            checked={row.selected ?? false}
                            disabled={disabled || !row.controllable}
                            onChange={(e) => {
                              const bindings =
                                preview?.policy.operationBindings[operation] ??
                                [];
                              onChange({
                                ...preview!.policy,
                                operationBindings: {
                                  ...preview!.policy.operationBindings,
                                  [operation]: e.target.checked
                                    ? [
                                        ...bindings.filter(
                                          (b) => b.app !== row.app,
                                        ),
                                        {
                                          app: row.app,
                                          fingerprint: row.bindingFingerprint,
                                        },
                                      ]
                                    : bindings.filter((b) => b.app !== row.app),
                                },
                              });
                            }}
                          />
                        ) : null}
                        {row.app} ·{" "}
                        {row.execution === "supported"
                          ? lang === 0
                            ? "账号证据支持"
                            : "Supported by account evidence"
                          : row.execution === "unsupported"
                            ? lang === 0
                              ? "当前账号不支持"
                              : "Unsupported by the current account"
                            : row.controllable
                              ? lang === 0
                                ? "可拦截，账号待实测"
                                : "Controllable; live verification pending"
                              : lang === 0
                                ? "当前运行时未实现或未启用"
                                : "Not implemented or enabled on this runtime"}{" "}
                        · {row.nativeUnit} ·{" "}
                        {row.pricing === "explicit_free"
                          ? lang === 0
                            ? "资源价格明确为零"
                            : "Resource rate explicitly zero"
                          : row.pricing === "priced"
                            ? lang === 0
                              ? "已配置资源价"
                              : "Resource price configured"
                            : lang === 0
                              ? "模型范围或资源价格未覆盖"
                              : "Model range or resource pricing incomplete"}
                      </p>
                    ))
                  : lang === 0
                    ? "无可用运行时"
                    : "No available runtime"}
              </div>
            </div>
          );
        })}
      </div>
      <details>
        <summary className="cursor-pointer text-sm">
          {lang === 0 ? "用户有效能力与权限范围" : "User capability scopes"}
        </summary>
        <div className="mt-2 grid gap-3">
          {Object.entries(grants)
            .filter(([, g]) => g.active)
            .map(([email, g]) => (
              <div key={email} className="text-xs">
                <span>{email}</span>
                {g.manager === "routerShareMarket" ? (
                  <span>
                    {" "}
                    · {lang === 0 ? "市场合同固定" : "Frozen Market contract"}
                  </span>
                ) : (
                  <>
                    <select
                      className="ml-2 rounded border p-1"
                      aria-label={`${email} capabilities`}
                      value={
                        g.policy.allowedOperations === undefined
                          ? "inherit"
                          : "explicit"
                      }
                      disabled={disabled}
                      onChange={(e) =>
                        onGrantOperationsChange(
                          email,
                          e.target.value === "inherit" ? undefined : [],
                        )
                      }
                    >
                      <option value="inherit">
                        {lang === 0 ? "继承 Share" : "Inherit Share"}
                      </option>
                      <option value="explicit">
                        {lang === 0 ? "仅允许勾选项" : "Explicit subset"}
                      </option>
                    </select>
                    {g.policy.allowedOperations !== undefined ? (
                      <div className="mt-1 flex flex-wrap gap-2">
                        {selected.map((op) => (
                          <label key={op} className="flex gap-1">
                            <input
                              type="checkbox"
                              checked={g.policy.allowedOperations?.includes(op)}
                              disabled={disabled}
                              onChange={(e) =>
                                onGrantOperationsChange(
                                  email,
                                  e.target.checked
                                    ? [...g.policy.allowedOperations!, op]
                                    : g.policy.allowedOperations!.filter(
                                        (v) => v !== op,
                                      ),
                                )
                              }
                            />
                            {OPERATION_LABELS[op][lang]}
                          </label>
                        ))}
                      </div>
                    ) : null}
                  </>
                )}
              </div>
            ))}
        </div>
      </details>
      <details>
        <summary className="cursor-pointer text-xs">
          {lang === 0 ? "批量修改能力权限" : "Batch capability permissions"}
        </summary>
        <div className="my-2 flex flex-wrap gap-3 text-xs">
          {Object.entries(grants)
            .filter(([, g]) => g.active && g.manager !== "routerShareMarket")
            .map(([email]) => (
              <label key={email} className="flex gap-1">
                <input
                  type="checkbox"
                  checked={bulkEmails.includes(email)}
                  disabled={disabled}
                  onChange={(e) =>
                    setBulkEmails((current) =>
                      e.target.checked
                        ? [...current, email]
                        : current.filter((v) => v !== email),
                    )
                  }
                />
                {email}
              </label>
            ))}
        </div>
        <div className="my-2 flex flex-wrap gap-3 text-xs">
          {selected.map((op) => (
            <label key={op} className="flex gap-1">
              <input
                type="checkbox"
                checked={bulkOps.includes(op)}
                disabled={disabled}
                onChange={(e) =>
                  setBulkOps((current) =>
                    e.target.checked
                      ? [...current, op]
                      : current.filter((v) => v !== op),
                  )
                }
              />
              {OPERATION_LABELS[op][lang]}
            </label>
          ))}
        </div>
        <div className="flex gap-3 text-xs">
          <button
            type="button"
            disabled={disabled || !bulkEmails.length}
            className="underline"
            onClick={() =>
              bulkEmails
                .filter(
                  (e) =>
                    grants[e]?.active &&
                    grants[e].manager !== "routerShareMarket",
                )
                .forEach((email) => onGrantOperationsChange(email, undefined))
            }
          >
            {lang === 0 ? "选中用户继承 Share" : "Selected users inherit Share"}
          </button>
          <button
            type="button"
            disabled={disabled || !bulkEmails.length}
            className="underline"
            onClick={() =>
              bulkEmails
                .filter(
                  (e) =>
                    grants[e]?.active &&
                    grants[e].manager !== "routerShareMarket",
                )
                .forEach((email) =>
                  onGrantOperationsChange(
                    email,
                    bulkOps.filter((op) => selected.includes(op)),
                  ),
                )
            }
          >
            {lang === 0
              ? "选中用户仅允许勾选项（空表示禁用）"
              : "Limit selected users to checked operations (empty disables)"}
          </button>
        </div>
      </details>
      {preview?.blockers?.some(
        (b) => b.blocker === "VALUE_QUOTA_RESOURCE_POLICY_REQUIRED",
      ) ? (
        <button
          type="button"
          className="text-left text-xs underline"
          disabled={disabled}
          onClick={() => onChange(policy ?? emptyCapabilityPolicy())}
        >
          {lang === 0
            ? "为选中能力采用资源价卡并重新预览（保留缺价策略）"
            : "Use resource tariff and preview again (preserves missing-price policy)"}
        </button>
      ) : null}
      {preview?.blockers?.map((b, index) => (
        <p
          key={`${b.scope}-${index}`}
          role="alert"
          className="text-xs text-red-700"
        >
          {b.scope}:{" "}
          {b.blocker === "VALUE_QUOTA_RESOURCE_POLICY_REQUIRED"
            ? lang === 0
              ? "需要先为美元授权采用资源价卡，再刷新预览。"
              : "USD grants require a resource tariff before refreshing the preview."
            : b.blocker}
        </p>
      ))}
      <details>
        <summary className="cursor-pointer text-xs">
          {lang === 0 ? "额度覆盖与计量说明" : "Quota coverage and measurement"}
        </summary>
        {preview?.rows
          .filter((row) => selected.includes(row.operation))
          .map((row) => (
            <div key={`${row.app}-${row.operation}`} className="my-2 text-xs">
              <p>
                {row.app} · {OPERATION_LABELS[row.operation][lang]} ·{" "}
                {lang === 0
                  ? "数量可能估算，预占是软上限"
                  : "Quantities may be estimated; reservation is a soft limit"}
              </p>
              {row.scopes.map((scope) => (
                <p key={scope.scope}>
                  {scope.scope} · {scope.quotaUnit} · {scope.coverage}:{" "}
                  {COVERAGE_LABELS[scope.reason]?.[lang] ?? scope.reason}
                </p>
              ))}
            </div>
          ))}
      </details>
      {required.length ? (
        <div className="grid max-h-64 gap-2 overflow-y-auto rounded-lg bg-amber-50 p-3 text-xs">
          <p>
            {lang === 0
              ? "以下消耗无法完整约束到所选额度。逐项确认仅适用于当前用户、单位和运行时；切换后需重新预览。"
              : "The selected quotas do not fully cover these resources. Acceptance is scoped to the current user, unit and runtime."}
          </p>
          {required.map((ack) => {
            const detail = preview?.rows
              .flatMap((row) => row.scopes)
              .find((scope) => scope.condition === ack.condition);
            return (
              <label key={ack.condition} className="flex items-start gap-2">
                <input
                  type="checkbox"
                  checked={accepted.includes(ack.condition)}
                  disabled={disabled}
                  onChange={(e) =>
                    setAccepted((current) =>
                      e.target.checked
                        ? [...current, ack.condition]
                        : current.filter((c) => c !== ack.condition),
                    )
                  }
                />
                <span>
                  {ack.scope === "$default"
                    ? lang === 0
                      ? "默认访问与 owner"
                      : "Default access and owner"
                    : ack.scope}{" "}
                  · {OPERATION_LABELS[ack.operation][lang]} · {ack.quotaUnit}:{" "}
                  {COVERAGE_LABELS[detail?.reason ?? ""]?.[lang] ??
                    detail?.reason}
                </span>
              </label>
            );
          })}
        </div>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="outline"
          isDisabled={disabled}
          onClick={() => setRefresh((v) => v + 1)}
        >
          {lang === 0 ? "刷新预览" : "Refresh preview"}
        </Button>
        <Button
          size="sm"
          isDisabled={
            disabled ||
            !preview?.ready ||
            expired ||
            (preview?.blockers?.length ?? 0) > 0 ||
            required.some((ack) => !accepted.includes(ack.condition))
          }
          onClick={() => {
            if (preview?.previewCredential)
              onChange(preview.policy, {
                credential: preview.previewCredential,
                acceptUncovered: accepted,
              });
          }}
        >
          {lang === 0 ? "确认本次能力配置" : "Confirm capability configuration"}
        </Button>
      </div>
      {preview?.incidents?.length ? (
        <div role="alert" className="rounded border border-red-300 p-2 text-xs">
          <p>
            {lang === 0
              ? "发现未计划的资源执行，受影响的用户与运行时已隔离。需修复控制路径后升级执行版本或重新绑定，再预览确认；历史用量与证据保留。"
              : "Unplanned resource execution quarantined the affected user and runtime. Repair the control path, upgrade its execution version or rebind, then preview again. Historical usage and evidence are retained."}
          </p>
          {preview.incidents.map((item) => (
            <p key={`${item.requestId}-${item.resource}`}>
              {item.scope} · {item.resource} ·{" "}
              {item.bindingFingerprint.slice(0, 12)}
            </p>
          ))}
        </div>
      ) : null}
      {preview?.recentUsage?.length ? (
        <details>
          <summary className="cursor-pointer text-xs">
            {lang === 0
              ? "最近执行与冻结计量明细"
              : "Recent execution and frozen metering"}
          </summary>
          <div className="max-h-64 space-y-2 overflow-y-auto">
            {preview.recentUsage.map((item) => (
              <div
                className="rounded border p-2 text-xs"
                key={`${item.requestId}-${item.attempt}`}
              >
                <p>
                  {item.scope} · #{item.attempt} ·{" "}
                  {item.meteringStatus === "not_in_usd_ledger"
                    ? lang === 0
                      ? "未记录美元扣减"
                      : "No USD quota debit recorded"
                    : item.meteringStatus}{" "}
                  ·{" "}
                  {item.unpricedReason
                    ? lang === 0
                      ? "部分已计费"
                      : "Partially billed"
                    : lang === 0
                      ? "等值计量金额"
                      : "Equivalent metered value"}
                  : {capabilityAmount(item.settledPicos)}
                </p>
                {item.dimensions.map((d) => (
                  <p key={d.dimension}>
                    {d.dimension}: {d.quantity} · {d.pricing} ·{" "}
                    {capabilityAmount(d.amountPicos)}
                    {item.estimated
                      ? lang === 0
                        ? "（估算）"
                        : " (estimated)"
                      : null}
                  </p>
                ))}
                <p className="break-all text-muted-foreground">
                  {item.pricingRevision}
                </p>
              </div>
            ))}
          </div>
        </details>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? "确认后，与本页其他更改一起保存。能力变更不会清零已消耗额度。"
          : "After confirming, save together with the other edits on this page. Capability changes preserve consumed usage."}
      </p>
    </section>
  );
}
