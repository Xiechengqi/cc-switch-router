import * as React from "react";
import {
  capabilityQuotaContext,
  capabilityRiskGroups,
  capabilitySelectable,
  type CapabilityApproval,
  type CapabilityPreview,
  type CapabilityReviewHandle,
  type ShareCapabilityPolicy,
} from "./share-capabilities";

export type CapabilityReviewPrompt = {
  preview: CapabilityPreview;
  groups: ReturnType<typeof capabilityRiskGroups>;
  migration: boolean;
};

/** Read-only previews remain separate from deliberate consent and the final settings transaction. */
export function useCapabilityReview({
  shareId,
  revision,
  policy,
  savedPolicy,
  grants,
  tokenLimit,
  request,
  reviewRef,
  lang,
}: {
  shareId?: string;
  revision?: number;
  policy?: ShareCapabilityPolicy;
  savedPolicy?: ShareCapabilityPolicy;
  grants: Parameters<typeof capabilityQuotaContext>[1];
  tokenLimit: number;
  request: (
    shareId: string,
    input: unknown,
    signal?: AbortSignal,
  ) => Promise<CapabilityPreview>;
  reviewRef?: React.Ref<CapabilityReviewHandle>;
  lang: 0 | 1;
}) {
  const key = JSON.stringify([
    shareId,
    revision,
    capabilityQuotaContext(policy, grants, tokenLimit),
  ]);
  const keyRef = React.useRef(key);
  keyRef.current = key;
  const [result, setResult] = React.useState<{
    key: string;
    preview: CapabilityPreview;
  }>();
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");
  const [refresh, setRefresh] = React.useState(0);
  const [prompt, setPrompt] = React.useState<CapabilityReviewPrompt>();
  const [accepted, setAccepted] = React.useState<string[]>([]);
  const resolveRef = React.useRef<
    ((value: CapabilityApproval | null) => void) | null
  >(null);
  const reviewAbort = React.useRef<AbortController | null>(null);
  const input = () => ({
    expectedConfigRevision: revision,
    replacePolicies: true,
    policy,
    policies: Object.fromEntries(
      Object.entries(grants)
        .filter(([, g]) => g.active)
        .map(([email, g]) => [email, g.policy]),
    ),
    shareTokenLimit: tokenLimit,
  });
  const cancel = React.useCallback(() => {
    reviewAbort.current?.abort();
    resolveRef.current?.(null);
    resolveRef.current = null;
    setPrompt(undefined);
    setAccepted([]);
  }, []);
  React.useEffect(() => {
    cancel();
    setError("");
    if (!shareId || revision == null) {
      setResult(undefined);
      setLoading(false);
      return;
    }
    const abort = new AbortController();
    setLoading(true);
    const timer = setTimeout(() => {
      void request(shareId, input(), abort.signal)
        .then((preview) => {
          if (!abort.signal.aborted && keyRef.current === key)
            setResult({ key, preview });
        })
        .catch((reason) => {
          if (!abort.signal.aborted && keyRef.current === key)
            setError(reason instanceof Error ? reason.message : String(reason));
        })
        .finally(() => {
          if (!abort.signal.aborted && keyRef.current === key)
            setLoading(false);
        });
    }, 200);
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
    // Audit fields and callback identities do not change execution intent.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, refresh, cancel]);
  React.useEffect(
    () => () => {
      reviewAbort.current?.abort();
      resolveRef.current?.(null);
      resolveRef.current = null;
    },
    [],
  );
  React.useImperativeHandle(
    reviewRef,
    () => ({
      review: async () => {
        if (!shareId || revision == null)
          throw new Error(
            lang === 0
              ? "请先创建 Share，再设置扩展能力。"
              : "Create the Share before configuring extensions.",
          );
        cancel();
        const abort = new AbortController();
        reviewAbort.current = abort;
        setError("");
        try {
          let preview = await request(shareId, input(), abort.signal);
          const displayPreview = preview;
          if (abort.signal.aborted || keyRef.current !== key) return null;
          // A known App whose account/runtime changed needs deliberate reauthorization.
          // Never authorize a newly added App through an existing binding's consent.
          const rebound: {
            operation: ShareCapabilityPolicy["allowedOperations"][number];
            app: string;
          }[] = [];
          if (policy) {
            const bindings = { ...policy.operationBindings };
            for (const op of policy.allowedOperations) {
              if (!bindings[op]?.length) continue;
              bindings[op] = bindings[op]!.map((binding) => {
                const row = preview.rows.find(
                  (row) =>
                    row.operation === op &&
                    row.app === binding.app &&
                    capabilitySelectable(row),
                );
                if (!row || row.bindingFingerprint === binding.fingerprint)
                  return binding;
                rebound.push({ operation: op, app: row.app });
                return { app: row.app, fingerprint: row.bindingFingerprint };
              });
            }
            if (rebound.length) {
              preview = await request(
                shareId,
                {
                  ...input(),
                  policy: { ...policy, operationBindings: bindings },
                },
                abort.signal,
              );
              if (abort.signal.aborted || keyRef.current !== key) return null;
            }
          }
          setResult({ key, preview: displayPreview });
          if (
            !preview.ready ||
            !preview.previewCredential ||
            preview.expiresAt <= Date.now()
          ) {
            throw new Error(
              lang === 0
                ? "能力信息尚未就绪，请检查 Server 连接与版本后重试。"
                : "Capability information is not ready. Check the Server connection and version, then retry.",
            );
          }
          if (preview.blockers?.length) {
            const market = preview.blockers.some(
              (b) => b.blocker === "MARKET_CAPABILITY_CONFLICT",
            );
            throw new Error(
              lang === 0
                ? market
                  ? "已有购买套餐限制本次能力变更，请先处理相关套餐。"
                  : "部分美元授权尚未采用资源价目，请重新检查相关用户设置。"
                : market
                  ? "An existing purchased plan prevents this capability change."
                  : "Some USD grants require the resource tariff. Check their settings.",
            );
          }
          const groups = capabilityRiskGroups(preview, savedPolicy);
          if (rebound.length)
            groups.push({
              key: "runtime_binding_changed",
              reason: "runtime_binding_changed",
              quotaUnit: "",
              operations: [...new Set(rebound.map((item) => item.operation))],
              scopes: [...new Set(rebound.map((item) => item.app))],
              conditions: [],
            });
          const migration = !savedPolicy && !!policy;
          if (!groups.length && !migration)
            return {
              policy: preview.policy,
              consent: {
                credential: preview.previewCredential,
                acceptUncovered: [],
              },
            };
          return new Promise<CapabilityApproval | null>((resolve) => {
            resolveRef.current = resolve;
            setAccepted([]);
            setPrompt({ preview, groups, migration });
          });
        } catch (reason) {
          if (abort.signal.aborted || keyRef.current !== key) return null;
          setError(reason instanceof Error ? reason.message : String(reason));
          throw reason;
        }
      },
    }),
    [key, request, lang, savedPolicy, cancel],
  );
  const confirm = () => {
    if (!prompt || prompt.groups.some((group) => !accepted.includes(group.key)))
      return;
    if (
      !prompt.preview.previewCredential ||
      prompt.preview.expiresAt <= Date.now()
    ) {
      setError(
        lang === 0
          ? "检查结果已过期，请关闭确认后重新保存。"
          : "The review expired. Close it and save again.",
      );
      cancel();
      return;
    }
    resolveRef.current?.({
      policy: prompt.preview.policy,
      consent: {
        credential: prompt.preview.previewCredential,
        acceptUncovered: [
          ...new Set(prompt.groups.flatMap((group) => group.conditions)),
        ],
      },
    });
    resolveRef.current = null;
    setPrompt(undefined);
    setAccepted([]);
  };
  return {
    preview:
      result && result.preview.shareId === shareId ? result.preview : undefined,
    current: result?.key === key,
    loading,
    error,
    prompt,
    accepted,
    setAccepted,
    cancel,
    confirm,
    refresh: () => setRefresh((v) => v + 1),
  };
}
