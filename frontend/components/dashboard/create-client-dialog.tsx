"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Dices, ExternalLink, Loader2, LogIn, Minus, Plus } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  checkClientTunnelSubdomainAvailability,
  createClientMarketClient,
  getClientMarketJob,
  getClientMarketSupplySummary,
} from "@/lib/api";
import type { CreateClientRegionsPersist, CreateClientSelectionPersist, SupplySummaryEntry } from "@/lib/types";
import { usePersistentState } from "@/lib/use-persistent-state";

const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";
const HOST_OWNERS_KEY = "cc_switch_router_create_client_host_owners_v1";
const REGIONS_KEY = "cc_switch_router_create_client_regions_v1";

function randomSubdomain() {
  const letters = "abcdefghijklmnopqrstuvwxyz";
  const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789";
  let out = letters[Math.floor(Math.random() * letters.length)];
  for (let i = 1; i < 10; i++) out += alphabet[Math.floor(Math.random() * alphabet.length)];
  return out;
}

function uniqueOwners(entries: SupplySummaryEntry[]) {
  return Array.from(new Set(entries.map((e) => e.hostOwnerEmail))).sort((a, b) => a.localeCompare(b));
}

function aggregateRegions(entries: SupplySummaryEntry[], ownerEmails: string[]) {
  const ownerSet = new Set(ownerEmails.map((e) => e.toLowerCase()));
  const map = new Map<string, { idle: number; total: number }>();
  for (const entry of entries) {
    if (!ownerSet.has(entry.hostOwnerEmail.toLowerCase())) continue;
    const code = (entry.countryCode || "").trim().toUpperCase();
    if (!code) continue;
    const prev = map.get(code) || { idle: 0, total: 0 };
    prev.idle += entry.idleCount;
    prev.total += entry.totalCount;
    map.set(code, prev);
  }
  return Array.from(map.entries())
    .map(([code, counts]) => ({ code, ...counts }))
    .sort((a, b) => a.code.localeCompare(b.code));
}

function normalizeOwnerPersist(value: unknown): CreateClientSelectionPersist {
  if (!value || typeof value !== "object") return { mode: "all", emails: [] };
  const candidate = value as Partial<CreateClientSelectionPersist>;
  const emails = Array.isArray(candidate.emails)
    ? candidate.emails.filter((email): email is string => typeof email === "string")
    : [];
  return { mode: candidate.mode === "subset" ? "subset" : "all", emails };
}

function normalizeRegionPersist(value: unknown): CreateClientRegionsPersist {
  if (!value || typeof value !== "object") return { mode: "all", codes: [] };
  const candidate = value as Partial<CreateClientRegionsPersist>;
  const codes = Array.isArray(candidate.codes)
    ? candidate.codes.filter((code): code is string => typeof code === "string")
    : [];
  return { mode: candidate.mode === "subset" ? "subset" : "all", codes };
}

type Phase = "form" | "running" | "success" | "failed";

export function CreateClientDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { locale, t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;

  const [supply, setSupply] = React.useState<SupplySummaryEntry[]>([]);
  const [supplyLoading, setSupplyLoading] = React.useState(false);
  const [hostOwnersPersist, setHostOwnersPersist, ownersHydrated] = usePersistentState<CreateClientSelectionPersist>(
    HOST_OWNERS_KEY,
    { mode: "all", emails: [] },
  );
  const [regionsPersist, setRegionsPersist, regionsHydrated] = usePersistentState<CreateClientRegionsPersist>(
    REGIONS_KEY,
    { mode: "all", codes: [] },
  );

  const [subdomain, setSubdomain] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [quantity, setQuantity] = React.useState(1);
  const [pendingLogin, setPendingLogin] = React.useState(false);
  const [phase, setPhase] = React.useState<Phase>("form");
  const [jobLog, setJobLog] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [successInfo, setSuccessInfo] = React.useState<{
    subdomain?: string;
    installationId?: string;
    hostOwnerEmail?: string;
    clientOwnerEmail?: string;
    countryCode?: string;
    clientUrl?: string;
  }>({});
  const supplyReadyRef = React.useRef(false);
  const ownersReconciledRef = React.useRef(false);
  const regionsReconciledRef = React.useRef(false);
  const previousOwnerSignatureRef = React.useRef("");
  const pollGenerationRef = React.useRef(0);

  const allOwners = React.useMemo(() => uniqueOwners(supply), [supply]);
  const safeHostOwnersPersist = React.useMemo(
    () => normalizeOwnerPersist(hostOwnersPersist),
    [hostOwnersPersist],
  );
  const safeRegionsPersist = React.useMemo(
    () => normalizeRegionPersist(regionsPersist),
    [regionsPersist],
  );

  const resolvedOwnerEmails = React.useMemo(() => {
    if (safeHostOwnersPersist.mode === "all") return allOwners;
    return safeHostOwnersPersist.emails.filter((e) => allOwners.some((o) => o.toLowerCase() === e.toLowerCase()));
  }, [allOwners, safeHostOwnersPersist]);

  const regionOptions = React.useMemo(
    () => aggregateRegions(supply, resolvedOwnerEmails),
    [resolvedOwnerEmails, supply],
  );

  const resolvedCountryCodes = React.useMemo(() => {
    if (safeRegionsPersist.mode === "all") return regionOptions.map((r) => r.code);
    return safeRegionsPersist.codes.filter((c) => regionOptions.some((r) => r.code === c));
  }, [regionOptions, safeRegionsPersist]);

  const ownerSignature = React.useMemo(
    () => resolvedOwnerEmails.map((email) => email.toLowerCase()).sort().join("\n"),
    [resolvedOwnerEmails],
  );
  const selectedIdleCapacity = React.useMemo(() => {
    const selected = new Set(resolvedCountryCodes);
    return regionOptions.reduce((total, region) => total + (selected.has(region.code) ? region.idle : 0), 0);
  }, [regionOptions, resolvedCountryCodes]);
  const regionNames = React.useMemo(
    () => new Intl.DisplayNames([locale], { type: "region" }),
    [locale],
  );

  React.useEffect(() => {
    if (!open || !ownersHydrated || !supplyReadyRef.current || ownersReconciledRef.current) return;
    if (allOwners.length === 0) return;
    const intersection = safeHostOwnersPersist.emails.filter((email) =>
      allOwners.some((available) => available.toLowerCase() === email.toLowerCase()),
    );
    ownersReconciledRef.current = true;
    if (safeHostOwnersPersist.mode === "all" || intersection.length === 0) {
      setHostOwnersPersist({ mode: "all", emails: allOwners });
    } else if (intersection.length !== safeHostOwnersPersist.emails.length) {
      setHostOwnersPersist({ mode: "subset", emails: intersection });
    }
  }, [allOwners, open, ownersHydrated, safeHostOwnersPersist, setHostOwnersPersist]);

  React.useEffect(() => {
    if (
      !open ||
      !regionsHydrated ||
      !ownersReconciledRef.current ||
      !supplyReadyRef.current
    ) return;
    const options = regionOptions.map((region) => region.code);
    const intersection = safeRegionsPersist.codes.filter((code) => options.includes(code));
    const ownerChanged = previousOwnerSignatureRef.current !== ownerSignature;
    previousOwnerSignatureRef.current = ownerSignature;
    if (!regionsReconciledRef.current) {
      if (options.length === 0) return;
      regionsReconciledRef.current = true;
      if (safeRegionsPersist.mode === "all" || intersection.length === 0) {
        setRegionsPersist({ mode: "all", codes: options });
      } else if (intersection.length !== safeRegionsPersist.codes.length) {
        setRegionsPersist({ mode: "subset", codes: intersection });
      }
      return;
    }
    if (safeRegionsPersist.mode === "all") {
      if (options.join("\n") !== safeRegionsPersist.codes.join("\n")) {
        setRegionsPersist({ mode: "all", codes: options });
      }
      return;
    }
    if (options.length === 0) return;
    if (intersection.length === 0 && (ownerChanged || safeRegionsPersist.codes.length > 0)) {
      setRegionsPersist({ mode: "all", codes: options });
    } else if (intersection.length !== safeRegionsPersist.codes.length) {
      setRegionsPersist({ mode: "subset", codes: intersection });
    }
  }, [open, ownerSignature, regionOptions, regionsHydrated, safeRegionsPersist, setRegionsPersist]);

  React.useEffect(() => {
    if (!open) return;
    supplyReadyRef.current = false;
    ownersReconciledRef.current = false;
    regionsReconciledRef.current = false;
    previousOwnerSignatureRef.current = "";
    pollGenerationRef.current += 1;
    setPhase("form");
    setJobLog("");
    setError("");
    setSuccessInfo({});
    setPassword("");
    setSubdomain(randomSubdomain());
    setQuantity(1);
    setPendingLogin(false);
    setSupplyLoading(true);
    getClientMarketSupplySummary()
      .then((entries) => {
        supplyReadyRef.current = true;
        setSupply(entries);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setSupplyLoading(false));
  }, [open]);

  React.useEffect(() => {
    if (open) return;
    pollGenerationRef.current += 1;
    setPassword("");
  }, [open]);

  React.useEffect(() => {
    if (!pendingLogin || !authed) return;
    setPendingLogin(false);
  }, [authed, pendingLogin]);

  const toggleOwner = (email: string) => {
    const set = new Set(safeHostOwnersPersist.mode === "all" ? allOwners : safeHostOwnersPersist.emails);
    if (set.has(email)) set.delete(email);
    else set.add(email);
    setHostOwnersPersist({ mode: "subset", emails: Array.from(set).sort((a, b) => a.localeCompare(b)) });
  };

  const toggleRegion = (code: string) => {
    const set = new Set(safeRegionsPersist.mode === "all" ? regionOptions.map((region) => region.code) : safeRegionsPersist.codes);
    if (set.has(code)) set.delete(code);
    else set.add(code);
    setRegionsPersist({ mode: "subset", codes: Array.from(set).sort((a, b) => a.localeCompare(b)) });
  };

  const pollJob = async (jobId: string, generation: number) => {
    for (let i = 0; i < 1000; i++) {
      await new Promise((r) => setTimeout(r, 1500));
      if (pollGenerationRef.current !== generation) return;
      let job;
      try {
        job = await getClientMarketJob(jobId);
        setError("");
      } catch {
        setError(t("createClient.statusRetrying"));
        continue;
      }
      setJobLog(job.log || "");
      if (job.status === "succeeded") {
        setSuccessInfo({
          subdomain: job.subdomain,
          installationId: job.installationId,
          hostOwnerEmail: job.hostOwnerEmail,
          clientOwnerEmail: job.clientOwnerEmail,
          countryCode: job.countryCode,
          clientUrl: job.clientUrl,
        });
        setPhase("success");
        return;
      }
      if (job.status === "failed") {
        setPhase("failed");
        setError(t("createClient.failed"));
        return;
      }
    }
    setPhase("failed");
    setError(t("createClient.failed"));
  };

  const onPrimary = async () => {
    if (!authed) {
      setPendingLogin(true);
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    if (resolvedOwnerEmails.length === 0 || resolvedCountryCodes.length === 0) {
      setError(t("createClient.selectionRequired"));
      return;
    }
    if (selectedIdleCapacity < quantity) {
      setError(t("createClient.noCapacity"));
      return;
    }
    if (quantity > 1) {
      toast.info(t("createClient.batchSoon"));
      return;
    }
    if (password.length < 8) {
      setError(t("createClient.passwordHint"));
      return;
    }
    const nextSubdomain = subdomain.trim();
    if (!nextSubdomain) {
      setError(t("createClient.subdomainRequired"));
      return;
    }
    setBusy(true);
    setError("");
    try {
      const availability = await checkClientTunnelSubdomainAvailability(nextSubdomain);
      if (!availability.available) {
        setError(t("createClient.subdomainTaken"));
        setBusy(false);
        return;
      }
      const { jobId } = await createClientMarketClient({
        hostOwnerEmails: resolvedOwnerEmails,
        countryCodes: resolvedCountryCodes,
        subdomain: nextSubdomain,
        password,
        count: 1,
      });
      setPassword("");
      setPhase("running");
      setJobLog("");
      const generation = pollGenerationRef.current;
      await pollJob(jobId, generation);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setPhase("form");
    } finally {
      setBusy(false);
    }
  };

  const primaryLabel = !authed ? t("createClient.login") : phase === "running" ? t("createClient.provisioning") : t("createClient.create");
  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen && phase === "running") return;
    onOpenChange(nextOpen);
  };

  return (
    <Modal.Backdrop
      isOpen={open}
      onOpenChange={handleOpenChange}
      isDismissable={phase !== "running"}
    >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading className="!text-slate-900">{t("createClient.title")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[min(70vh,560px)] gap-4 overflow-y-auto !text-slate-900">
              {phase === "success" ? (
                <div className="grid gap-2 rounded-lg border border-emerald-200 bg-emerald-50 p-4 text-sm">
                  <p className="font-semibold text-emerald-900">{t("createClient.successTitle")}</p>
                  <p>
                    <span className="text-muted-foreground">{t("createClient.successSubdomain")}: </span>
                    <span className="font-mono">{successInfo.subdomain}</span>
                  </p>
                  <p>
                    <span className="text-muted-foreground">{t("createClient.successInstallation")}: </span>
                    <span className="font-mono text-xs">{successInfo.installationId}</span>
                  </p>
                  {successInfo.hostOwnerEmail ? (
                    <p>
                      <span className="text-muted-foreground">{t("createClient.successHostOwner")}: </span>
                      {successInfo.hostOwnerEmail}
                    </p>
                  ) : null}
                  {successInfo.clientOwnerEmail ? (
                    <p>
                      <span className="text-muted-foreground">{t("createClient.successClientOwner")}: </span>
                      {successInfo.clientOwnerEmail}
                    </p>
                  ) : null}
                  {successInfo.countryCode ? (
                    <p className="flex items-center gap-2">
                      <span className="text-muted-foreground">{t("createClient.successRegion")}: </span>
                      <CountryFlag code={successInfo.countryCode} className="h-3.5 w-5 rounded-sm object-cover" />
                      <span>{regionNames.of(successInfo.countryCode) || successInfo.countryCode}</span>
                    </p>
                  ) : null}
                  {successInfo.clientUrl ? (
                    <a
                      href={successInfo.clientUrl}
                      target="_blank"
                      rel="noreferrer"
                      className="inline-flex min-w-0 items-center gap-1.5 font-medium text-primary hover:underline"
                    >
                      <span className="truncate">{successInfo.clientUrl}</span>
                      <ExternalLink className="h-3.5 w-3.5 shrink-0" />
                    </a>
                  ) : null}
                </div>
              ) : phase === "running" || phase === "failed" ? (
                <div className="grid gap-2">
                  <div className="font-mono text-[10px] uppercase text-muted-foreground">
                    {t("createClient.log")}
                  </div>
                  <pre className="max-h-48 overflow-auto rounded-lg border bg-slate-950 p-3 font-mono text-[11px] leading-5 text-slate-100">
                    {jobLog || "…"}
                  </pre>
                  {error ? (
                    <p className={phase === "failed" ? "text-sm text-rose-600" : "text-sm text-amber-600"}>
                      {error}
                    </p>
                  ) : null}
                </div>
              ) : (
                <>
                  {supplyLoading ? (
                    <div className="flex items-center gap-2 text-sm text-muted-foreground">
                      <Loader2 className="h-4 w-4 animate-spin" />
                      …
                    </div>
                  ) : null}
                  <section className="grid gap-2">
                    <div className="text-sm font-medium text-slate-900">{t("createClient.hostOwners")}</div>
                    <div className="flex flex-wrap gap-2">
                      <Button
                        size="sm"
                        variant={safeHostOwnersPersist.mode === "all" ? "primary" : "outline"}
                        onClick={() => setHostOwnersPersist({ mode: "all", emails: allOwners })}
                      >
                        {t("createClient.hostOwnersAll")}
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => setHostOwnersPersist({ mode: "subset", emails: [] })}
                      >
                        {t("createClient.clearOwners")}
                      </Button>
                    </div>
                    <div className="grid max-h-32 gap-1 overflow-y-auto rounded-lg border p-2 text-slate-900">
                      {allOwners.length === 0 ? (
                        <p className="px-1 py-2 text-xs text-muted-foreground">—</p>
                      ) : (
                        allOwners.map((email) => {
                          const checked =
                            safeHostOwnersPersist.mode === "all" ||
                            safeHostOwnersPersist.emails.some((selected) => selected.toLowerCase() === email.toLowerCase());
                          return (
                            <label key={email} className="flex cursor-pointer items-center gap-2 text-sm text-slate-900">
                              <input
                                type="checkbox"
                                checked={checked}
                                onChange={() => {
                                  if (safeHostOwnersPersist.mode === "all") {
                                    setHostOwnersPersist({
                                      mode: "subset",
                                      emails: allOwners.filter((item) => item !== email),
                                    });
                                    return;
                                  }
                                  toggleOwner(email);
                                }}
                              />
                              <span className="truncate">{email}</span>
                            </label>
                          );
                        })
                      )}
                    </div>
                  </section>

                  <section className="grid gap-2">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="text-sm font-medium text-slate-900">{t("createClient.regions")}</div>
                      <div className="flex flex-wrap gap-2">
                        <Button
                          size="sm"
                          variant={safeRegionsPersist.mode === "all" ? "primary" : "outline"}
                          onClick={() => setRegionsPersist({ mode: "all", codes: regionOptions.map((r) => r.code) })}
                        >
                          {t("createClient.regionsAll")}
                        </Button>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => setRegionsPersist({ mode: "subset", codes: [] })}
                        >
                          {t("createClient.clearRegions")}
                        </Button>
                      </div>
                    </div>
                    <div className="grid max-h-44 gap-1 overflow-y-auto rounded-lg border p-2 text-slate-900">
                      {regionOptions.length === 0 ? (
                        <p className="px-1 py-2 text-xs text-muted-foreground">—</p>
                      ) : (
                        regionOptions.map((region) => {
                          const checked =
                            safeRegionsPersist.mode === "all" || safeRegionsPersist.codes.includes(region.code);
                          return (
                            <label key={region.code} className="flex cursor-pointer items-center gap-2 rounded-md px-1 py-1 text-sm text-slate-900 hover:bg-slate-50">
                              <input
                                type="checkbox"
                                checked={checked}
                                onChange={() => {
                                  if (safeRegionsPersist.mode === "all") {
                                    const next = regionOptions.map((r) => r.code).filter((c) => c !== region.code);
                                    setRegionsPersist({ mode: "subset", codes: next });
                                    return;
                                  }
                                  toggleRegion(region.code);
                                }}
                              />
                              <CountryFlag code={region.code} className="h-3.5 w-5 rounded-sm object-cover" />
                              <span className="min-w-0 flex-1 truncate font-medium">
                                {regionNames.of(region.code) || region.code}
                              </span>
                              <span className="shrink-0 font-mono text-xs text-muted-foreground">
                                {region.idle}/{region.total}
                              </span>
                            </label>
                          );
                        })
                      )}
                    </div>
                  </section>

                  <section className="grid gap-2">
                    <div className="text-sm font-medium text-slate-900">{t("createClient.quantity")}</div>
                    <div className="inline-flex items-center gap-2">
                      <Button
                        size="sm"
                        variant="outline"
                        isIconOnly
                        className="h-8 w-8 min-w-8 rounded-md p-0"
                        isDisabled={quantity <= 1 || phase === "running"}
                        aria-label={t("createClient.decreaseQuantity")}
                        onClick={() => setQuantity((q) => Math.max(1, q - 1))}
                      >
                        <Minus className="h-4 w-4" />
                      </Button>
                      <span className="min-w-8 text-center font-mono text-sm text-slate-900">{quantity}</span>
                      <Button
                        size="sm"
                        variant="outline"
                        isIconOnly
                        className="h-8 w-8 min-w-8 rounded-md p-0"
                        isDisabled={quantity >= Math.max(1, selectedIdleCapacity) || phase === "running"}
                        aria-label={t("createClient.increaseQuantity")}
                        onClick={() =>
                          setQuantity((q) => Math.min(Math.max(1, selectedIdleCapacity), q + 1))
                        }
                      >
                        <Plus className="h-4 w-4" />
                      </Button>
                    </div>
                    <span className="text-xs text-muted-foreground">
                      {t("createClient.capacity", { idle: selectedIdleCapacity })}
                    </span>
                  </section>

                  <label className="grid gap-1 text-sm">
                    <span className="font-medium text-slate-900">{t("createClient.subdomain")}</span>
                    <div className="flex items-center gap-2">
                      <input
                        value={quantity > 1 ? t("createClient.subdomainRandom") : subdomain}
                        onChange={(e) => {
                          if (quantity > 1) return;
                          setSubdomain(e.target.value);
                        }}
                        readOnly={quantity > 1}
                        disabled={quantity > 1}
                        className="h-11 min-w-0 flex-1 rounded-lg border border-border bg-white px-3 font-mono text-slate-900 outline-none focus:ring-2 focus:ring-primary/30 disabled:cursor-not-allowed disabled:bg-slate-50 disabled:text-slate-500"
                        autoComplete="off"
                      />
                      {quantity === 1 ? (
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          className="h-11 shrink-0 gap-1.5 px-3"
                          aria-label={t("createClient.randomSubdomain")}
                          onClick={() => setSubdomain(randomSubdomain())}
                        >
                          <Dices className="h-4 w-4" />
                          {t("createClient.randomSubdomain")}
                        </Button>
                      ) : null}
                    </div>
                  </label>
                  <label className="grid gap-1 text-sm">
                    <span className="font-medium text-slate-900">{t("createClient.password")}</span>
                    <input
                      type="password"
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      className="h-11 rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                      autoComplete="new-password"
                    />
                    <span className="text-xs text-muted-foreground">{t("createClient.passwordHint")}</span>
                  </label>
                  {error ? <p className="text-sm text-rose-600">{error}</p> : null}
                </>
              )}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={phase === "running"} onClick={() => handleOpenChange(false)}>
                {phase === "success" ? t("createClient.close") : t("common.close")}
              </Button>
              {phase === "form" || phase === "running" ? (
                <Button
                  variant="primary"
                  isDisabled={
                    busy ||
                    authLoading ||
                    phase === "running" ||
                    (authed && selectedIdleCapacity < quantity)
                  }
                  onClick={() => void onPrimary()}
                >
                  {!authed ? <LogIn className="h-4 w-4" /> : busy || phase === "running" ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : null}
                  {primaryLabel}
                </Button>
              ) : null}
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
    </Modal.Backdrop>
  );
}
