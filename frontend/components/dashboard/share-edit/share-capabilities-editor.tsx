"use client";
/* Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V3 */
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { InfoHint } from "@/components/common/info-hint";
import { previewShareCapabilities } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  capabilityAmount,
  capabilityPresentation,
  capabilityWarnings,
  capabilitySelectable,
  capabilityStatus,
  capabilityPricingText,
  capabilityScopeLabel,
  OPERATION_LABELS,
  COVERAGE_LABELS,
  emptyCapabilityPolicy,
  type ShareCapabilityPolicy,
  type CapabilityReviewHandle,
  type CapabilityConsent,
  type ShareOperation,
} from "@/lib/share-capabilities";
import { useCapabilityReview } from "@/lib/use-share-capabilities";
import type { ShareUserGrantMap } from "@/lib/types";

/** Share-wide intent only; individual restrictions live in the user quota editor. */
export function ShareCapabilitiesEditor({
  shareId,
  revision,
  policy,
  savedPolicy,
  grants,
  tokenLimit,
  disabled,
  onChange,
  previewCapabilities = previewShareCapabilities,
  reviewRef,
}: {
  shareId: string;
  revision: number;
  policy?: ShareCapabilityPolicy;
  savedPolicy?: ShareCapabilityPolicy;
  grants: ShareUserGrantMap;
  tokenLimit: number;
  disabled?: boolean;
  onChange: (
    policy: ShareCapabilityPolicy,
    consent?: CapabilityConsent,
  ) => void;
  previewCapabilities?: typeof previewShareCapabilities;
  reviewRef?: React.Ref<CapabilityReviewHandle>;
}) {
  const { locale } = useLocaleText();
  const lang = locale.startsWith("zh") ? 0 : 1;
  const review = useCapabilityReview({
    shareId,
    revision,
    policy,
    savedPolicy,
    grants,
    tokenLimit,
    request: previewCapabilities,
    reviewRef,
    lang,
  });
  const { preview } = review;
  const view = capabilityPresentation({
    ...review,
    policy,
    savedPolicy,
    reviewing: !!review.prompt,
  });
  const { main, setup } = view;
  const ready =
    review.current && preview?.ready && !review.loading && !review.error;
  const choose = (operation: ShareOperation, checked: boolean) => {
    const current = policy ?? emptyCapabilityPolicy();
    const bindings = { ...current.operationBindings };
    delete bindings[operation];
    onChange({
      ...current,
      operationBindings: bindings,
      allowedOperations: checked
        ? [...current.allowedOperations, operation]
        : current.allowedOperations.filter((op) => op !== operation),
    });
  };
  // Keep the review hook mounted even when there is no visible configuration.
  if (view.mode === "hidden") return null;
  const compact = view.mode === "compact";
  const title = !compact
    ? lang === 0
      ? "扩展能力"
      : "Extended capabilities"
    : view.status === "loading"
      ? lang === 0
        ? "正在检查扩展能力…"
        : "Checking extensions…"
      : view.status === "unavailable"
        ? view.blocked
          ? lang === 0
            ? "扩展能力暂不可保存"
            : "Extension changes unavailable"
          : lang === 0
            ? "扩展能力暂无法读取"
            : "Extensions unavailable"
        : lang === 0
          ? "扩展能力需配置"
          : "Extensions need setup";
  return (
    <section
      className={`grid min-w-0 gap-2 [overflow-wrap:anywhere] ${compact ? "py-1" : "rounded-xl border border-border p-4"}`}
      aria-label={lang === 0 ? "扩展能力" : "Extended capabilities"}
      data-capability-view={view.mode}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <h3
            className={
              compact ? "text-xs text-muted-foreground" : "font-medium"
            }
            role={
              view.status === "unavailable"
                ? "alert"
                : view.status === "loading"
                  ? "status"
                  : undefined
            }
          >
            {title}
          </h3>
          <InfoHint label={lang === 0 ? "扩展能力说明" : "About extensions"}>
            <span className="grid gap-1.5">
              <span>
                {lang === 0
                  ? "控制此 Share 的生成能力和供应商工具。图片、文档、音频和视频理解输入不受影响。"
                  : "Controls generation and Provider-hosted tools. Image, document, audio and video understanding inputs are unaffected."}
              </span>
              <span>
                {lang === 0
                  ? "用户默认跟随 Share，可在“用户授权与配额”中进一步限制。"
                  : "Users follow the Share by default. Restrict individuals in user access and quotas."}
              </span>
              <span>
                {lang === 0
                  ? "与本页其他修改一起保存，已消耗额度保留。美元为 API 等值额度。"
                  : "Saved with this page; consumed quota is retained. USD means API-equivalent quota."}
              </span>
              {!policy ? (
                <span>
                  {lang === 0
                    ? "开始配置并保存后，未选择的扩展能力将关闭。"
                    : "Once configured and saved, unselected extensions will be disabled."}
                </span>
              ) : null}
              {view.status === "unavailable" ? (
                <span>
                  {view.blocked
                    ? lang === 0
                      ? "能力变更暂不可保存。请检查协议、存储或处理授权冲突后重试。"
                      : "Changes cannot be saved yet. Check protocol, storage or conflicting permissions."
                    : lang === 0
                      ? "读取不完整，不能据此判断供应商没有扩展能力。已有选择保留，请检查供应商配置、Server 连接、协议与存储后重试。"
                      : "The check is incomplete. Existing choices are retained. Check Provider settings, Server connectivity, protocol and storage, then retry."}
                </span>
              ) : null}
              {preview?.blockers?.some(
                (blocker) => blocker.blocker === "MARKET_CAPABILITY_CONFLICT",
              ) ? (
                <span>
                  {lang === 0
                    ? "已购买套餐限制本次变更，请先处理相关套餐。"
                    : "An existing purchased plan prevents these changes. Resolve the affected plan first."}
                </span>
              ) : null}
              {preview?.blockers?.some(
                (blocker) =>
                  blocker.blocker === "VALUE_QUOTA_RESOURCE_POLICY_REQUIRED",
              ) ? (
                <span>
                  {lang === 0
                    ? "相关美元授权需要先采用资源价目，请检查用户授权与配额。"
                    : "Some USD grants require the resource tariff. Check user access and quotas."}
                </span>
              ) : null}
              {preview && preview.displayVersion !== 2 ? (
                <span>
                  {lang === 0
                    ? "请升级 Server 以读取完整能力信息。"
                    : "Upgrade the Server for complete capability information."}
                </span>
              ) : null}
              {preview?.unavailableBindings?.map((binding) => (
                <span key={binding.app}>
                  {binding.app} ·{" "}
                  {lang === 0
                    ? "供应商绑定无法读取"
                    : "Provider binding unavailable"}
                </span>
              ))}
              {review.error ? (
                <span className="break-all">{review.error}</span>
              ) : null}
            </span>
          </InfoHint>
        </div>
        <Button
          size="sm"
          variant="ghost"
          isDisabled={disabled || review.loading}
          onClick={review.refresh}
        >
          {view.status === "unavailable" && !view.blocked
            ? lang === 0
              ? "重试"
              : "Retry"
            : lang === 0
              ? "刷新"
              : "Refresh"}
        </Button>
      </div>
      {!compact && view.status === "loading" ? (
        <p role="status" className="text-xs text-muted-foreground">
          {lang === 0
            ? "正在检查当前供应商能力…"
            : "Checking current Provider capabilities…"}
        </p>
      ) : null}
      {!compact && view.status === "unavailable" ? (
        <p role="alert" className="text-xs text-muted-foreground">
          {view.blocked
            ? lang === 0
              ? "能力变更暂不可保存。"
              : "Extension changes cannot be saved yet."
            : lang === 0
              ? "部分能力暂无法读取，已有选择保留。"
              : "Some extensions are unavailable. Existing choices are retained."}
        </p>
      ) : null}
      {!compact && !policy ? (
        <div className="grid justify-items-start gap-2 text-sm">
          <Button
            size="sm"
            variant="outline"
            isDisabled={disabled || !ready}
            onClick={() => onChange(preview?.policy ?? emptyCapabilityPolicy())}
          >
            {lang === 0
              ? "设置扩展能力权限"
              : "Configure extension permissions"}
          </Button>
        </div>
      ) : !compact && policy ? (
        <>
          {view.pending && !main.length ? (
            <p className="text-xs text-muted-foreground">
              {lang === 0
                ? "能力变更待保存。"
                : "Extension changes await saving."}
            </p>
          ) : null}
          {main.map((option) => (
            <div
              key={option.operation}
              className="min-w-0 border-t border-border py-2"
            >
              <label className="flex cursor-pointer items-center justify-between gap-3 text-sm">
                <span className="flex min-w-0 items-center gap-2">
                  <input
                    type="checkbox"
                    checked={option.selected}
                    disabled={
                      disabled ||
                      !ready ||
                      (!option.available && !option.selected)
                    }
                    onChange={(event) =>
                      choose(option.operation, event.target.checked)
                    }
                    className="accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 disabled:cursor-not-allowed"
                  />
                  {OPERATION_LABELS[option.operation][lang]}
                </span>
                <span className="text-xs text-muted-foreground">
                  {!option.available
                    ? lang === 0
                      ? "暂不可用"
                      : "Unavailable"
                    : option.rows.some(
                          (row) =>
                            capabilitySelectable(row) &&
                            row.accountSupport !== "supported",
                        )
                      ? lang === 0
                        ? "账号待验证"
                        : "Verification pending"
                      : lang === 0
                        ? "账号已验证"
                        : "Account verified"}
                </span>
              </label>
              {option.selected &&
              capabilityWarnings(option.rows, lang).length ? (
                <div className="mt-1 flex items-center gap-1.5 pl-6 text-xs font-medium text-foreground">
                  <span>
                    {capabilityWarnings(option.rows, lang)
                      .map((warning) => warning.label)
                      .join(" · ")}
                  </span>
                  <InfoHint
                    label={
                      lang === 0 ? "额度计量提醒" : "Quota metering notice"
                    }
                  >
                    <span className="grid gap-1.5">
                      {capabilityWarnings(option.rows, lang).map((warning) => (
                        <span key={warning.reason}>{warning.detail}</span>
                      ))}
                    </span>
                  </InfoHint>
                </div>
              ) : null}
              <details className="mt-1 pl-6 text-xs text-muted-foreground">
                <summary className="cursor-pointer">
                  {lang === 0 ? "查看支持与计量" : "Support and metering"}
                </summary>
                <div className="mt-2 grid gap-2">
                  {option.rows
                    .filter(
                      (row) =>
                        row.implemented !== false ||
                        row.selected ||
                        option.selected,
                    )
                    .map((row) => (
                      <div key={row.app} className="grid gap-1">
                        <p>
                          {row.providerLabel ||
                            (lang === 0
                              ? "当前供应商"
                              : "Current Provider")}{" "}
                          · {row.app} · {capabilityStatus(row, lang)}
                        </p>
                        {row.model ? (
                          <p>
                            {lang === 0 ? "固定模型：" : "Fixed model: "}
                            {row.model}
                          </p>
                        ) : (
                          <p>
                            {lang === 0
                              ? "模型由实际请求决定"
                              : "Model determined by each request"}
                          </p>
                        )}
                        <p>{capabilityPricingText(row, lang)}</p>
                        {option.selected &&
                        (option.rows.filter(capabilitySelectable).length > 1 ||
                          option.rows.some(
                            (item) =>
                              capabilitySelectable(item) && !item.selected,
                          )) ? (
                          <label className="flex items-center gap-2">
                            <input
                              type="checkbox"
                              checked={row.selected ?? false}
                              disabled={
                                disabled || !ready || !capabilitySelectable(row)
                              }
                              aria-label={`${OPERATION_LABELS[option.operation][lang]} ${row.app}`}
                              onChange={(event) => {
                                if (!preview) return;
                                const current =
                                  preview.policy.operationBindings[
                                    option.operation
                                  ] ?? [];
                                onChange({
                                  ...preview.policy,
                                  operationBindings: {
                                    ...preview.policy.operationBindings,
                                    [option.operation]: event.target.checked
                                      ? [
                                          ...current.filter(
                                            (binding) =>
                                              binding.app !== row.app,
                                          ),
                                          {
                                            app: row.app,
                                            fingerprint: row.bindingFingerprint,
                                          },
                                        ]
                                      : current.filter(
                                          (binding) => binding.app !== row.app,
                                        ),
                                  },
                                });
                              }}
                            />
                            {lang === 0
                              ? `允许 ${row.app} 入口`
                              : `Allow ${row.app} access`}
                          </label>
                        ) : null}
                        {option.selected && row.selected ? (
                          <div className="grid gap-1">
                            {[
                              ...new Map(
                                row.scopes.map((scope) => [
                                  JSON.stringify([
                                    scope.quotaUnit,
                                    scope.reason,
                                  ]),
                                  scope,
                                ]),
                              ).values(),
                            ].map((scope) => (
                              <p key={`${scope.quotaUnit}-${scope.reason}`}>
                                {COVERAGE_LABELS[scope.reason]?.[lang] ??
                                  (lang === 0
                                    ? "计量状态尚未明确"
                                    : "Measurement status unknown")}
                              </p>
                            ))}
                          </div>
                        ) : null}
                      </div>
                    ))}
                  {!option.rows.length ? (
                    <p>
                      {lang === 0
                        ? "原绑定已不可用，可取消此能力。"
                        : "The previous binding is unavailable. You can disable this capability."}
                    </p>
                  ) : null}
                </div>
              </details>
            </div>
          ))}
        </>
      ) : null}
      {setup.length ? (
        <details className="text-xs text-muted-foreground">
          <summary className="cursor-pointer">
            {lang === 0 ? "配置说明" : "Setup instructions"}
          </summary>
          <div className="mt-2 grid gap-1">
            {setup.map((option) => (
              <p key={option.operation}>
                {OPERATION_LABELS[option.operation][lang]} ·{" "}
                {lang === 0
                  ? "需先在供应商设置中开启"
                  : "Enable in Provider settings first"}
              </p>
            ))}
          </div>
        </details>
      ) : null}
      {preview?.incidents?.length ? (
        <div role="alert" className="rounded border border-danger p-2 text-xs">
          <p>
            {lang === 0
              ? "发现异常资源执行，相关调用已隔离。请先修复执行路径并重新核对绑定。"
              : "Unexpected resource execution quarantined affected calls. Repair the execution path and review its binding."}
          </p>
          <details>
            <summary>{lang === 0 ? "异常证据" : "Incident evidence"}</summary>
            {preview.incidents.map((item) => (
              <p key={`${item.requestId}-${item.resource}`}>
                {capabilityScopeLabel(item.scope, lang)} · {item.resource} ·{" "}
                {item.bindingFingerprint.slice(0, 12)}
              </p>
            ))}
          </details>
        </div>
      ) : null}
      {preview?.recentUsage?.length ? (
        <details className="text-xs">
          <summary className="cursor-pointer">
            {lang === 0
              ? "最近执行与计量明细"
              : "Recent execution and metering"}
          </summary>
          <div className="mt-2 grid max-h-64 gap-2 overflow-y-auto">
            {preview.recentUsage.map((item) => (
              <div
                key={`${item.requestId}-${item.attempt}`}
                className="border-t pt-2"
              >
                <p>
                  {capabilityScopeLabel(item.scope, lang)} · #{item.attempt} ·{" "}
                  {item.meteringStatus === "not_in_usd_ledger"
                    ? lang === 0
                      ? "未记录美元扣减"
                      : "No USD quota debit recorded"
                    : `${item.meteringStatus} · ${capabilityAmount(item.settledPicos)}`}
                </p>
                {item.estimated ? (
                  <p>
                    {lang === 0
                      ? "资源数量包含估算"
                      : "Resource quantities include estimates"}
                  </p>
                ) : null}
                {item.unpricedReason ? (
                  <p>
                    {lang === 0
                      ? "含未定价用量："
                      : "Includes unpriced usage: "}
                    {item.unpricedReason}
                  </p>
                ) : null}
                {item.pendingRiskPicos && item.pendingRiskPicos !== "0" ? (
                  <p>
                    {lang === 0
                      ? "待结算风险预占："
                      : "Pending risk reservation: "}
                    {capabilityAmount(item.pendingRiskPicos)}
                  </p>
                ) : null}
                {item.dimensions.map((dimension) => (
                  <p key={dimension.dimension}>
                    {dimension.dimension} · {dimension.quantity} ·{" "}
                    {dimension.pricing === "not_in_usd_ledger"
                      ? lang === 0
                        ? "未记录美元扣减"
                        : "No USD quota debit recorded"
                      : dimension.pricing === "missing"
                        ? lang === 0
                          ? "未定价"
                          : "Unpriced"
                        : capabilityAmount(dimension.amountPicos)}
                  </p>
                ))}
                {item.pricingRevision ? (
                  <p className="break-all text-muted-foreground">
                    {lang === 0 ? "冻结价目版本：" : "Frozen tariff revision: "}
                    {item.pricingRevision}
                  </p>
                ) : null}
              </div>
            ))}
          </div>
        </details>
      ) : null}
      <Modal.Backdrop
        isOpen={!!review.prompt}
        onOpenChange={(open) => {
          if (!open) review.cancel();
        }}
        className="z-[90]"
      >
        <Modal.Container placement="center" className="z-[90]">
          <Modal.Dialog className="share-edit-surface light w-[min(560px,calc(100vw-2rem))] max-w-none max-h-[85dvh] overflow-y-auto bg-background text-foreground [overflow-wrap:anywhere]">
            <Modal.Header>
              <Modal.Heading>
                {lang === 0 ? "确认扩展能力变更" : "Review extension changes"}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-4">
              {review.prompt?.migration ? (
                <p className="text-sm">
                  {lang === 0
                    ? "保存后开始独立控制扩展能力；未选择的能力将关闭。"
                    : "Saving activates independent permissions. Unselected extensions will be disabled."}
                </p>
              ) : null}
              <p className="text-sm">
                {lang === 0 ? "允许：" : "Allow: "}
                {review.prompt?.preview.policy.allowedOperations
                  .map((op) => OPERATION_LABELS[op][lang])
                  .join(lang === 0 ? "、" : ", ") ||
                  (lang === 0 ? "无扩展能力" : "No extensions")}
              </p>
              {review.prompt?.groups.map((group) => (
                <div
                  key={group.key}
                  className="grid gap-2 border-t pt-3 text-sm"
                >
                  <p>
                    {COVERAGE_LABELS[group.reason]?.[lang] ??
                      (lang === 0
                        ? "部分资源无法完整计量或扣减额度。"
                        : "Some resources cannot be fully measured or debited.")}
                  </p>
                  <p className="text-xs text-muted-foreground">
                    {group.operations
                      .map((op) => OPERATION_LABELS[op][lang])
                      .join(lang === 0 ? "、" : ", ")}
                  </p>
                  <details className="text-xs">
                    <summary className="cursor-pointer">
                      {lang === 0
                        ? "查看受影响的用户与访问范围"
                        : "Affected users and access"}
                    </summary>
                    {group.scopes.map((scope) => (
                      <p className="break-all" key={scope}>
                        {capabilityScopeLabel(scope, lang)}
                      </p>
                    ))}
                  </details>
                  <label className="flex items-start gap-2">
                    <input
                      type="checkbox"
                      checked={review.accepted.includes(group.key)}
                      onChange={(event) =>
                        review.setAccepted((current) =>
                          event.target.checked
                            ? [...current, group.key]
                            : current.filter((key) => key !== group.key),
                        )
                      }
                    />
                    {group.reason === "runtime_binding_changed"
                      ? lang === 0
                        ? "为当前绑定重新授权"
                        : "Authorize the current binding"
                      : lang === 0
                        ? "我了解并允许上述未覆盖消耗"
                        : "I understand and allow this uncovered usage"}
                  </label>
                </div>
              ))}
              <p className="text-xs text-muted-foreground">
                {lang === 0
                  ? "美元指 API 等值额度。估算与预占不能保证上游执行不会超量。"
                  : "USD means API-equivalent quota. Estimates and reservations cannot guarantee an upstream execution limit."}
              </p>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" onClick={review.cancel}>
                {lang === 0 ? "返回修改" : "Back"}
              </Button>
              <Button
                isDisabled={review.prompt?.groups.some(
                  (group) => !review.accepted.includes(group.key),
                )}
                onClick={review.confirm}
              >
                {lang === 0 ? "确认并继续保存" : "Confirm and save"}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
