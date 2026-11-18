"use client";
/* Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V3 */
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { previewShareCapabilities } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  capabilityAmount,
  capabilityOptions,
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
  const options = capabilityOptions(preview, policy);
  const main = options.filter((option) => option.available || option.selected);
  const setup = options.filter(
    (option) => !option.available && !option.selected && option.configurable,
  );
  const ready = review.current && preview?.ready && !review.loading;
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
  const following = Object.values(grants).filter(
    (g) =>
      g.active &&
      g.role !== "owner" &&
      g.policy.allowedOperations === undefined &&
      g.manager !== "routerShareMarket",
  ).length;
  return (
    <section
      className="grid min-w-0 gap-3 rounded-xl border border-border p-4 [overflow-wrap:anywhere]"
      aria-label={lang === 0 ? "扩展能力" : "Extended capabilities"}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="font-medium">
            {lang === 0 ? "扩展能力" : "Extended capabilities"}
          </h3>
          <p className="mt-1 text-xs text-muted-foreground">
            {lang === 0
              ? "选择此 Share 允许的生成能力与供应商工具。图片、文档、音频和视频理解输入不受这些开关影响。"
              : "Choose generation capabilities and Provider-hosted tools. Image, document, audio and video understanding inputs are unaffected."}
          </p>
        </div>
        <Button
          size="sm"
          variant="ghost"
          isDisabled={disabled || review.loading}
          onClick={review.refresh}
        >
          {lang === 0 ? "刷新" : "Refresh"}
        </Button>
      </div>
      {review.error ? (
        <p role="alert" className="text-sm text-danger">
          {review.error}
        </p>
      ) : null}
      {review.loading ? (
        <p role="status" className="text-xs text-muted-foreground">
          {lang === 0
            ? "正在检查当前供应商能力…"
            : "Checking current Provider capabilities…"}
        </p>
      ) : null}
      {preview && !preview.ready ? (
        <p role="alert" className="text-xs text-warning">
          {lang === 0
            ? "当前无法保存能力变更，请检查 Server 连接、协议版本与存储状态。"
            : "Capability changes cannot be saved yet. Check the Server connection, protocol and storage."}
        </p>
      ) : null}
      {preview?.unavailableBindings?.length ? (
        <p role="alert" className="text-xs text-warning">
          {lang === 0
            ? "部分供应商绑定无法读取，已有选择保留；请处理供应商配置后重试。"
            : "Some Provider bindings are unavailable. Existing choices are retained; check Provider settings and retry."}
        </p>
      ) : null}
      {!policy ? (
        <div className="grid justify-items-start gap-2 text-sm">
          <p>
            {lang === 0
              ? "此 Share 尚未设置独立能力权限。"
              : "This Share has no independent capability policy yet."}
          </p>
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
          <p className="text-xs text-muted-foreground">
            {lang === 0
              ? "开始配置后，保存时会确认本次允许和关闭的能力。"
              : "After configuration, saving reviews which capabilities will be allowed or disabled."}
          </p>
        </div>
      ) : (
        <>
          {preview && !options.length ? (
            <p className="py-2 text-sm text-muted-foreground">
              {lang === 0
                ? "当前绑定没有可配置的扩展能力。"
                : "The current bindings have no configurable extensions."}
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
              <details className="mt-1 pl-6 text-xs text-muted-foreground">
                <summary className="cursor-pointer">
                  {lang === 0 ? "查看支持与计量" : "Support and metering"}
                </summary>
                <div className="mt-2 grid gap-2">
                  {option.rows
                    .filter((row) => row.implemented !== false || row.selected || option.selected)
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
          {setup.length ? (
            <details className="text-xs text-muted-foreground">
              <summary className="cursor-pointer">
                {lang === 0 ? "需先配置的能力" : "Capabilities requiring setup"}
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
          {policy.allowedOperations.length ? (
            <p className="text-xs text-muted-foreground">
              {lang === 0
                ? `分享用户默认跟随此设置${following ? `（当前 ${following} 人）` : ""}。单个用户可在“用户授权与配额”中进一步限制。`
                : `Shared users follow these settings by default${following ? ` (${following} users)` : ""}. Restrict individuals under user access and quotas.`}
            </p>
          ) : null}
        </>
      )}
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? "与本页其他修改一起保存，已消耗额度保留。"
          : "Saved with the rest of this page. Consumed quota is retained."}
      </p>
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
