"use client";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  OPERATION_LABELS,
  type ShareCapabilityPolicy,
} from "@/lib/share-capabilities";
export function ShareCapabilitySummary({
  policy,
}: {
  policy?: ShareCapabilityPolicy;
}) {
  const { locale } = useLocaleText();
  const zh = locale.startsWith("zh");
  return (
    <details
      className="text-[10px] text-muted-foreground"
      data-no-row-drawer
      onClick={(event) => event.stopPropagation()}
    >
      <summary className="cursor-pointer">
        {zh ? "扩展能力" : "Extended capabilities"} ·{" "}
        {policy
          ? policy.allowedOperations.length
            ? policy.allowedOperations
                .map((op) => OPERATION_LABELS[op][zh ? 0 : 1])
                .join("、")
            : zh
              ? "已关闭"
              : "Disabled"
          : zh
            ? "旧兼容策略"
            : "Legacy authorization"}
      </summary>
      <p className="mt-1">
        {zh
          ? "图片、文档、音频、视频理解输入按原模型能力处理。Token 限额不等于媒体资源预算；美元按实际模型与价格覆盖扣减。"
          : "Image, document, audio and video understanding follows the model. Token limits do not represent media budgets; USD coverage depends on the actual model and pricing."}
      </p>
    </details>
  );
}
