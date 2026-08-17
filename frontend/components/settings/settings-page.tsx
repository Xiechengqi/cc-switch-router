"use client";

import {
  AlertTriangle,
  CheckCircle2,
  CircleGauge,
  Database,
  Eye,
  KeyRound,
  Loader2,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Settings2,
  ShoppingBag,
  Wifi,
} from "lucide-react";
import { Alert, Button, Chip, Input, Switch, TextArea } from "@heroui/react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { AlertChannelsPanel } from "@/components/settings/alert-channels-panel";
import { AnnouncementPanel } from "@/components/settings/announcement-panel";
import { MapDisplayPanel } from "@/components/settings/map-display-panel";
import {
  settingsCategoryDescription,
  settingsCategoryLabel,
  settingsFieldDescription,
  settingsFieldLabel,
  settingsFieldPlaceholder,
  settingsGroupLabel,
  settingsValueSource,
} from "@/lib/settings-i18n";
import {
  ApiError,
  getMapDisplay,
  getProvisionSshKey,
  getSettings,
  saveSettings,
  updateMapDisplay,
  validateSettings,
} from "@/lib/api";
import {
  DEFAULT_MAP_DISPLAY,
  rebaseMapDisplaySettings,
  sameMapDisplaySettings,
  toMapDisplayUpdate,
} from "@/lib/map-display-settings";
import type {
  MapDisplaySettings,
  ProvisionSshKey,
  SettingValueEntry,
  SettingsCategory,
  SettingsCategoryId,
  SettingsField,
  SettingsSnapshot,
} from "@/lib/types";

type DirtyValue = string | boolean | null;
type ActiveSection = "overview" | SettingsCategoryId;
type Banner = { kind: "default" | "success" | "destructive" | "warning"; text: string };
type SettingsPanel = "map" | "announcement" | "channel_health" | "provision_key";
type SettingsSubsection = {
  id: string;
  label: string;
  groups?: string[];
  panel?: SettingsPanel;
  fieldCount?: number;
  dirtyCount: number;
};

const PANEL_SUBSECTION_IDS: Record<SettingsPanel, string> = {
  map: "panel:map",
  announcement: "panel:announcement",
  channel_health: "panel:channel_health",
  provision_key: "panel:provision_key",
};

const NOTIFICATION_SUBSECTION_IDS = {
  system: "notifications:system",
  business: "notifications:business",
  channels: "notifications:channels",
} as const;

const NOTIFICATION_SUBSECTION_GROUPS = {
  system: ["Alerting"],
  business: ["Client notifications"],
  channels: ["Email (Resend)", "Telegram alerts", "Telegram bot"],
} as const;

const CATEGORY_ICONS: Record<SettingsCategoryId, React.ComponentType<{ className?: string }>> = {
  general_display: Settings2,
  connectivity: Wifi,
  data_lifecycle: Database,
  identity_security: KeyRound,
  notifications: CheckCircle2,
  observability: CircleGauge,
  marketplace: ShoppingBag,
};

export function SettingsPage() {
  const { session, loading } = useAuth();
  const { t } = useLocaleText();
  const [snapshot, setSnapshot] = React.useState<SettingsSnapshot | null>(null);
  const [activeSection, setActiveSection] = React.useState<ActiveSection>("overview");
  const [activeSubsections, setActiveSubsections] = React.useState<Partial<Record<SettingsCategoryId, string>>>({});
  const [query, setQuery] = React.useState("");
  const [dirty, setDirty] = React.useState<Record<string, DirtyValue>>({});
  const [fieldErrors, setFieldErrors] = React.useState<Record<string, string[]>>({});
  const [busy, setBusy] = React.useState("");
  const [banner, setBanner] = React.useState<Banner | null>(null);
  const [mapSaved, setMapSaved] = React.useState<MapDisplaySettings>(DEFAULT_MAP_DISPLAY);
  const [mapDraft, setMapDraft] = React.useState<MapDisplaySettings>(DEFAULT_MAP_DISPLAY);
  const [mapLoading, setMapLoading] = React.useState(true);
  const [mapError, setMapError] = React.useState("");
  const [provisionSshKey, setProvisionSshKey] = React.useState<ProvisionSshKey | null>(null);
  const [provisionError, setProvisionError] = React.useState("");
  const [settingsRevision, setSettingsRevision] = React.useState(0);
  const subsectionPanelId = React.useId().replaceAll(":", "");

  const isAdmin = !!session?.isAdmin;
  const mapDirty = !sameMapDisplaySettings(mapDraft, mapSaved);
  const values = React.useMemo(
    () => Object.fromEntries((snapshot?.values || []).map((entry) => [entry.key, entry])),
    [snapshot],
  );
  const dirtyCount = Object.keys(dirty).length + (mapDirty ? 1 : 0);

  const loadSettings = React.useCallback(async ({ preserveBanner = false, manageBusy = true } = {}) => {
    if (manageBusy) setBusy((current) => current || "load");
    try {
      const next = await getSettings();
      setSnapshot(next);
      setDirty({});
      setFieldErrors({});
      setSettingsRevision((revision) => revision + 1);
      if (!preserveBanner) setBanner(null);
      return true;
    } catch (cause) {
      setBanner({ kind: "destructive", text: errorMessage(cause) });
      return false;
    } finally {
      if (manageBusy) setBusy("");
    }
  }, []);

  const loadMap = React.useCallback(async () => {
    setMapLoading(true);
    try {
      const next = await getMapDisplay();
      setMapSaved(next);
      setMapDraft(next);
      setMapError("");
    } catch (cause) {
      setMapError(errorMessage(cause));
    } finally {
      setMapLoading(false);
    }
  }, []);

  const loadProvisionKey = React.useCallback(async () => {
    try {
      setProvisionSshKey(await getProvisionSshKey());
      setProvisionError("");
    } catch (cause) {
      setProvisionError(errorMessage(cause));
    }
  }, []);

  React.useEffect(() => {
    if (!isAdmin) return;
    void loadSettings();
    void loadMap();
    void loadProvisionKey();
  }, [isAdmin, loadMap, loadProvisionKey, loadSettings]);

  if (loading) {
    return <main className="mx-auto w-[calc(100%-2rem)] max-w-7xl py-12 text-muted-foreground">{t("common.loadingSession")}</main>;
  }

  if (!isAdmin) {
    return (
      <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-4xl gap-4 py-12 text-foreground">
        <h1 className="font-display text-3xl">{t("settings.adminRequired")}</h1>
        <p className="text-muted-foreground">{t("settings.adminRequiredDesc")}</p>
      </main>
    );
  }

  const schema = snapshot?.schema;
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const activeCategory = activeSection === "overview" ? undefined : activeSection;
  const categoryFields = (schema?.fields || []).filter((field) => field.category === activeCategory);
  const subsections = activeCategory
    ? buildSettingsSubsections(activeCategory, categoryFields, values, dirty, mapDirty, t)
    : [];
  const requestedSubsection = activeCategory ? activeSubsections[activeCategory] : undefined;
  const activeSubsection = subsections.find((item) => item.id === requestedSubsection) || subsections[0];
  const activeSubsectionIndex = Math.max(0, subsections.findIndex((item) => item.id === activeSubsection?.id));
  const matchingFields = (schema?.fields || []).filter((field) => {
    if (!normalizedQuery) {
      return !!activeSubsection?.groups?.includes(field.group) && field.category === activeCategory;
    }
    return [
      field.key,
      settingsFieldLabel(t, field),
      settingsFieldDescription(t, field),
      settingsGroupLabel(t, field.group),
    ].some((value) => value.toLocaleLowerCase().includes(normalizedQuery));
  });
  const visibleFields = matchingFields.filter((field) => dependenciesSatisfied(field, schema?.fields || [], values, dirty));
  const groupedFields = groupFields(visibleFields);
  const keepGeneralPanelsMounted = activeSection === "general_display";
  const keepProvisionKeyMounted = activeSection === "marketplace";
  const keepChannelHealthMounted = activeSection === "notifications";

  return (
    <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-10 text-foreground">
      <header className="grid gap-4 border-b pb-5 pt-2 md:grid-cols-[minmax(0,1fr)_auto] md:items-end">
        <div>
          <h1 className="font-display text-3xl">{t("settings.title")}</h1>
          <p className="mt-2 max-w-3xl text-sm text-muted-foreground">{t("settings.workspaceDescription")}</p>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="outline" onClick={resetDraft} isDisabled={!!busy || dirtyCount === 0}>
            <RotateCcw className="h-4 w-4" />
            {t("common.reset")}
          </Button>
          <Button variant="outline" onClick={() => reloadAll()} isDisabled={!!busy}>
            {busy === "load" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
            {t("common.reload")}
          </Button>
          <Button variant="primary" onClick={() => void submit()} isDisabled={!!busy || dirtyCount === 0 || !snapshot}>
            {busy === "save" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            {dirtyCount ? t("common.saveWithCount", { count: dirtyCount }) : t("common.save")}
          </Button>
        </div>
      </header>

      {banner ? <Alert status={bannerStatus(banner.kind)} className="!text-slate-900">{banner.text}</Alert> : null}
      {snapshot?.pendingRestartKeys.length ? (
        <Alert status="warning" className="!text-slate-900">
          {t("settings.pendingRestart", { count: snapshot.pendingRestartKeys.length })}
        </Alert>
      ) : null}

      <section className="grid gap-5 lg:grid-cols-[230px_minmax(0,1fr)]">
        <aside className="h-fit rounded-md bg-muted/35 p-2 lg:sticky lg:top-4">
          <nav aria-label={t("settings.categoriesAria")} className="grid gap-1">
            <SettingsNavButton
              active={activeSection === "overview" && !normalizedQuery}
              label={t("settings.overview")}
              count={dirtyCount || undefined}
              icon={Eye}
              onClick={() => selectCategory("overview")}
            />
            {(schema?.categories || []).map((category) => (
              <SettingsNavButton
                key={category.id}
                active={activeSection === category.id && !normalizedQuery}
                label={settingsCategoryLabel(t, category)}
                count={(
                  categoryDirtyCount(category.id, schema?.fields || [], dirty)
                  + (category.id === "general_display" && mapDirty ? 1 : 0)
                ) || undefined}
                icon={CATEGORY_ICONS[category.id]}
                onClick={() => selectCategory(category.id)}
              />
            ))}
          </nav>
        </aside>

        <div className="min-w-0">
          <div className="relative mb-5">
            <Search className="pointer-events-none absolute left-3 top-1/2 z-10 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              aria-label={t("settings.search")}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("settings.searchPlaceholder")}
              className="pl-9"
            />
          </div>

          {!snapshot && busy === "load" ? (
            <div className="flex min-h-64 items-center justify-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("settings.loading")}
            </div>
          ) : activeSection === "overview" && !normalizedQuery ? (
            <SettingsOverview snapshot={snapshot} dirtyCount={dirtyCount} onSelect={selectCategory} />
          ) : (
            <div className="grid gap-8">
              <div>
                <h2 className="font-display text-2xl">
                  {normalizedQuery
                    ? t("settings.searchResults")
                    : settingsCategoryLabel(t, schema?.categories.find((item) => item.id === activeSection))}
                </h2>
                <p className="mt-2 text-sm text-muted-foreground">
                  {normalizedQuery
                    ? t("settings.searchResultCount", { count: visibleFields.length })
                    : settingsCategoryDescription(t, schema?.categories.find((item) => item.id === activeSection))}
                </p>
              </div>

              {!normalizedQuery && activeCategory && subsections.length ? (
                <SettingsSubnav
                  items={subsections}
                  value={activeSubsection?.id || ""}
                  panelId={subsectionPanelId}
                  onChange={(id) => selectSubsection(activeCategory, id)}
                />
              ) : null}

              <div
                id={!normalizedQuery && activeCategory ? subsectionPanelId : undefined}
                role={!normalizedQuery && activeCategory ? "tabpanel" : undefined}
                aria-labelledby={!normalizedQuery && activeCategory ? `${subsectionPanelId}-tab-${activeSubsectionIndex}` : undefined}
                className="grid gap-8"
              >
                {groupedFields.map(([group, fields]) => (
                  <section key={group} className="grid gap-4">
                    <div className="flex flex-wrap items-center justify-between gap-2 px-1">
                      <h3 className="text-lg font-semibold">{settingsGroupLabel(t, group)}</h3>
                      <span className="text-xs tabular-nums text-muted-foreground">{t("settings.fieldCount", { count: fields.length })}</span>
                    </div>
                    <div className="grid gap-1 rounded-md bg-muted/30 p-1">
                      {fields.map((field) => (
                        <SettingsFieldRow
                          key={field.key}
                          field={field}
                          entry={values[field.key]}
                          value={dirtyValue(field, values[field.key], dirty)}
                          dirty={Object.prototype.hasOwnProperty.call(dirty, field.key)}
                          errors={fieldErrors[field.key] || []}
                          t={t}
                          onChange={(value) => updateDirty(field, value)}
                        />
                      ))}
                    </div>
                  </section>
                ))}

                {!visibleFields.length && normalizedQuery ? (
                  <div className="rounded-md bg-muted/30 py-14 text-center text-sm text-muted-foreground">{t("settings.noSearchResults")}</div>
                ) : null}

                {keepGeneralPanelsMounted ? (
                  <div hidden={!!normalizedQuery || activeSubsection?.panel !== "map"}>
                    <MapDisplayPanel
                      canEdit
                      value={mapDraft}
                      onChange={setMapDraft}
                      dirty={mapDirty}
                      loading={mapLoading}
                    />
                    {mapError ? <Alert status="danger">{mapError}</Alert> : null}
                  </div>
                ) : null}

                {keepGeneralPanelsMounted ? (
                  <div hidden={!!normalizedQuery || activeSubsection?.panel !== "announcement"}>
                    <AnnouncementPanel />
                  </div>
                ) : null}

                {keepProvisionKeyMounted ? (
                  <div hidden={!!normalizedQuery || activeSubsection?.panel !== "provision_key"}>
                    <ProvisionKeyPanel value={provisionSshKey} error={provisionError} />
                  </div>
                ) : null}

                {keepChannelHealthMounted ? (
                  <div hidden={!!normalizedQuery || activeSubsection?.panel !== "channel_health"}>
                    <AlertChannelsPanel refreshToken={settingsRevision} />
                  </div>
                ) : null}
              </div>
            </div>
          )}
        </div>
      </section>
    </main>
  );

  function updateDirty(field: SettingsField, value: DirtyValue) {
    setFieldErrors((current) => {
      if (!current[field.key]) return current;
      const next = { ...current };
      delete next[field.key];
      return next;
    });
    setDirty((current) => {
      if (sameDirtyValue(field, value, baseValue(field, values[field.key]))) {
        const next = { ...current };
        delete next[field.key];
        return next;
      }
      return { ...current, [field.key]: value };
    });
  }

  function selectCategory(category: ActiveSection) {
    setActiveSection(category);
    setQuery("");
  }

  function selectSubsection(category: SettingsCategoryId, id: string) {
    setActiveSubsections((current) => ({ ...current, [category]: id }));
  }

  function revealFirstFieldError(errors: Record<string, string[]>) {
    const field = snapshot?.schema.fields.find((candidate) => errors[candidate.key]?.length);
    if (!field) return;
    setActiveSection(field.category);
    selectSubsection(field.category, subsectionIdForField(field));
    setQuery("");
  }

  function resetDraft() {
    setDirty({});
    setFieldErrors({});
    setMapDraft(mapSaved);
    setBanner(null);
  }

  function reloadAll() {
    void loadSettings();
    void loadMap();
    void loadProvisionKey();
  }

  async function submit() {
    if (!snapshot) return;
    setBusy("save");
    setBanner(null);
    setFieldErrors({});
    let settingsSaved = false;
    let settingsReloadFailed = false;
    try {
      const updates = buildUpdates(snapshot.schema.fields, dirty);
      let settingsResult: Awaited<ReturnType<typeof saveSettings>> | null = null;
      if (Object.keys(updates).length) {
        const validation = await validateSettings(snapshot.revision, updates);
        if (!validation.valid) {
          setFieldErrors(validation.fieldErrors);
          revealFirstFieldError(validation.fieldErrors);
          setBanner({
            kind: "destructive",
            text: validation.formErrors[0] || t("settings.validationFailed"),
          });
          return;
        }
        settingsResult = await saveSettings(snapshot.revision, updates);
        settingsSaved = true;
        setDirty({});
        setFieldErrors({});
        setSnapshot((current) => current ? { ...current, revision: settingsResult!.revision } : current);
        settingsReloadFailed = !(await loadSettings({ preserveBanner: true, manageBusy: false }));
      }
      if (mapDirty) {
        const savedMap = await updateMapDisplay(toMapDisplayUpdate(mapDraft, mapSaved));
        setMapSaved(savedMap);
        setMapDraft(savedMap);
      }
      setBanner({
        kind: settingsReloadFailed || settingsResult?.restartRequiredKeys.length ? "warning" : "success",
        text: settingsReloadFailed
          ? t("settings.savedReloadFailed")
          : settingsResult
            ? t("settings.saved", {
              updated: settingsResult.updatedKeys.length,
              unchanged: settingsResult.unchangedKeys.length,
              restartRequired: settingsResult.restartRequiredKeys.length,
            })
            : t("settings.mapSaved"),
      });
    } catch (cause) {
      if (cause instanceof ApiError && cause.code === "SETTINGS_REVISION_CONFLICT") {
        try {
          const latest = await getSettings();
          setSnapshot(latest);
          setDirty(reconcileDirty(latest, dirty));
          setFieldErrors({});
          setSettingsRevision((revision) => revision + 1);
          setBanner({ kind: "warning", text: t("settings.revisionConflict") });
        } catch (reloadCause) {
          setBanner({
            kind: "destructive",
            text: t("settings.conflictReloadFailed", { reason: errorMessage(reloadCause) }),
          });
        }
        return;
      }
      if (cause instanceof ApiError && cause.code === "MAP_DISPLAY_REVISION_CONFLICT") {
        try {
          const latest = await getMapDisplay();
          setMapSaved(latest);
          setMapDraft(rebaseMapDisplaySettings(latest, mapSaved, mapDraft));
          setMapError("");
          setActiveSection("general_display");
          selectSubsection("general_display", PANEL_SUBSECTION_IDS.map);
          setQuery("");
          setBanner({
            kind: "warning",
            text: settingsSaved
              ? t("settings.mapRevisionConflictAfterSettings")
              : t("settings.mapRevisionConflict"),
          });
        } catch (reloadCause) {
          setBanner({
            kind: "destructive",
            text: t("settings.conflictReloadFailed", { reason: errorMessage(reloadCause) }),
          });
        }
        return;
      }
      if (cause instanceof ApiError && cause.code === "SETTINGS_VALIDATION_FAILED") {
        const errors = cause.details?.fieldErrors;
        if (errors && typeof errors === "object") {
          const nextErrors = errors as Record<string, string[]>;
          setFieldErrors(nextErrors);
          revealFirstFieldError(nextErrors);
        }
      }
      setBanner({
        kind: "destructive",
        text: settingsSaved
          ? t("settings.savedWithMapError", { reason: errorMessage(cause) })
          : errorMessage(cause),
      });
    } finally {
      setBusy("");
    }
  }
}

function SettingsOverview({
  snapshot,
  dirtyCount,
  onSelect,
}: {
  snapshot: SettingsSnapshot | null;
  dirtyCount: number;
  onSelect: (category: SettingsCategoryId) => void;
}) {
  const { t } = useLocaleText();
  if (!snapshot) return null;
  return (
    <div className="grid gap-7">
      <section className="grid rounded-md bg-muted/35 sm:grid-cols-2">
        <OverviewStat label={t("settings.configuredFields")} value={snapshot.schema.fields.length} />
        <OverviewStat label={t("settings.pendingRestartLabel")} value={snapshot.pendingRestartKeys.length} tone={snapshot.pendingRestartKeys.length ? "warning" : "default"} />
      </section>

      {dirtyCount ? <Alert status="warning">{t("settings.unsavedChanges", { count: dirtyCount })}</Alert> : null}

      <section>
        <h2 className="font-display text-2xl">{t("settings.configurationDomains")}</h2>
        <div className="mt-4 grid gap-1 rounded-md bg-muted/30 p-1">
          {snapshot.schema.categories.map((category) => {
            const Icon = CATEGORY_ICONS[category.id];
            return (
              <button
                key={category.id}
                type="button"
                onClick={() => onSelect(category.id)}
                className="grid w-full gap-3 rounded-sm px-3 py-4 text-left transition-colors hover:bg-background/80 sm:grid-cols-[32px_minmax(0,1fr)_auto] sm:items-center"
              >
                <Icon className="h-5 w-5 text-muted-foreground" />
                <span className="min-w-0">
                  <span className="block font-medium">{settingsCategoryLabel(t, category)}</span>
                  <span className="mt-1 block text-sm text-muted-foreground">{settingsCategoryDescription(t, category)}</span>
                </span>
                <span className="text-xs tabular-nums text-muted-foreground">{t("settings.fieldCount", { count: category.fieldCount })}</span>
              </button>
            );
          })}
        </div>
      </section>

      <section className="grid gap-2 text-xs text-muted-foreground">
        <div><code>{snapshot.envPath}</code></div>
        <div>{t("settings.snapshotGenerated", { time: new Date(snapshot.generatedAt).toLocaleString() })}</div>
      </section>
    </div>
  );
}

function OverviewStat({ label, value, tone = "default" }: { label: string; value: number; tone?: "default" | "warning" }) {
  return (
    <div className="px-4 py-5">
      <div className={`text-2xl font-semibold tabular-nums ${tone === "warning" && value ? "text-amber-700" : ""}`}>{value}</div>
      <div className="mt-1 text-xs text-muted-foreground">{label}</div>
    </div>
  );
}

function SettingsNavButton({
  active,
  label,
  count,
  icon: Icon,
  onClick,
}: {
  active: boolean;
  label: string;
  count?: number;
  icon: React.ComponentType<{ className?: string }>;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex min-h-10 w-full items-center gap-2 rounded-sm px-2.5 text-left text-sm transition-colors ${active ? "bg-background font-medium text-foreground" : "text-muted-foreground hover:bg-background/70 hover:text-foreground"}`}
    >
      <Icon className="h-4 w-4 shrink-0" />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count ? <span className="text-xs tabular-nums">{count}</span> : null}
    </button>
  );
}

function SettingsSubnav({
  items,
  value,
  panelId,
  onChange,
}: {
  items: SettingsSubsection[];
  value: string;
  panelId: string;
  onChange: (id: string) => void;
}) {
  const { t } = useLocaleText();
  const listRef = React.useRef<HTMLDivElement>(null);
  const selectedIndex = Math.max(0, items.findIndex((item) => item.id === value));

  const selectAt = (index: number) => {
    const item = items[index];
    if (!item) return;
    onChange(item.id);
    listRef.current
      ?.querySelector<HTMLButtonElement>(`[data-settings-subsection-index="${index}"]`)
      ?.focus();
  };

  return (
    <div className="max-w-full overflow-x-auto pb-1">
      <div
        ref={listRef}
        role="tablist"
        aria-label={t("settings.groupsAria")}
        aria-orientation="horizontal"
        className="flex min-w-max gap-1 rounded-md bg-muted/40 p-1"
        onKeyDown={(event) => {
          if (event.key === "ArrowRight") {
            event.preventDefault();
            selectAt((selectedIndex + 1) % items.length);
          } else if (event.key === "ArrowLeft") {
            event.preventDefault();
            selectAt((selectedIndex - 1 + items.length) % items.length);
          } else if (event.key === "Home") {
            event.preventDefault();
            selectAt(0);
          } else if (event.key === "End") {
            event.preventDefault();
            selectAt(items.length - 1);
          }
        }}
      >
        {items.map((item, index) => {
          const selected = item.id === value;
          const fieldCountLabel = item.fieldCount == null
            ? ""
            : t("settings.fieldCount", { count: item.fieldCount });
          const dirtyCountLabel = item.dirtyCount
            ? t("settings.changedFieldCount", { count: item.dirtyCount })
            : "";
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              id={`${panelId}-tab-${index}`}
              data-settings-subsection-index={index}
              aria-controls={panelId}
              aria-selected={selected}
              aria-label={[item.label, fieldCountLabel, dirtyCountLabel].filter(Boolean).join(", ")}
              tabIndex={selected ? 0 : -1}
              onClick={() => onChange(item.id)}
              className={`relative grid min-h-[58px] min-w-[9rem] content-center rounded-sm px-3 py-2 text-left transition-colors after:absolute after:inset-x-3 after:bottom-0 after:h-0.5 after:rounded-full after:transition-colors ${selected
                ? "bg-background text-foreground after:bg-primary"
                : "text-muted-foreground after:bg-transparent hover:bg-background/60 hover:text-foreground"}`}
            >
              <span className="whitespace-nowrap text-sm font-medium">{item.label}</span>
              {fieldCountLabel || dirtyCountLabel ? (
                <span className="mt-1 flex items-center gap-2 whitespace-nowrap text-[11px] tabular-nums">
                  {fieldCountLabel ? <span>{fieldCountLabel}</span> : null}
                  {dirtyCountLabel ? <span className="font-medium text-amber-700">{dirtyCountLabel}</span> : null}
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function SettingsFieldRow({
  field,
  entry,
  value,
  dirty,
  errors,
  t,
  onChange,
}: {
  field: SettingsField;
  entry?: SettingValueEntry;
  value: DirtyValue;
  dirty: boolean;
  errors: string[];
  t: ReturnType<typeof useLocaleText>["t"];
  onChange: (value: DirtyValue) => void;
}) {
  const secretClearing = field.fieldType === "secret" && value === null;
  const inputValue = secretClearing ? "" : String(value ?? "");
  const rowState = errors.length
    ? "bg-danger/5 hover:bg-danger/10"
    : dirty
      ? "bg-primary/5 hover:bg-primary/10"
      : entry?.pendingRestart
        ? "bg-warning/10 hover:bg-warning/15"
        : "hover:bg-background/75";
  return (
    <div className={`grid gap-4 rounded-sm px-4 py-5 transition-colors md:grid-cols-[minmax(230px,0.9fr)_minmax(0,1.1fr)] ${rowState}`}>
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <label className="font-medium" htmlFor={field.key}>{settingsFieldLabel(t, field)}</label>
          {field.required ? <Chip size="sm" variant="soft">{t("common.required")}</Chip> : null}
          {field.restartRequired ? <Chip color="warning" size="sm" variant="soft">{t("common.restartRequired")}</Chip> : null}
          {field.risk === "critical" ? <Chip color="danger" size="sm" variant="soft">{t("settings.highRisk")}</Chip> : null}
          {dirty ? <Chip color="accent" size="sm" variant="soft">{t("common.changed")}</Chip> : null}
        </div>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{settingsFieldDescription(t, field)}</p>
        <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
          <code className="break-all">{field.key}</code>
          <span>·</span>
          <span>{t("settings.persistedSource", { source: settingsValueSource(t, entry?.source) })}</span>
          <span>·</span>
          <span>{t("settings.runtimeSource", { source: settingsValueSource(t, entry?.effectiveSource) })}</span>
          {entry?.pendingRestart ? <span className="text-amber-700">· {t("settings.pendingRestartShort")}</span> : null}
        </div>
        {entry?.pendingRestart && !entry?.isSecret ? (
          <p className="mt-2 text-xs text-muted-foreground">
            {t("settings.effectiveValue", { value: entry.effectiveValue || t("common.unset") })}
          </p>
        ) : null}
        {entry?.pendingRestart && entry?.isSecret ? (
          <p className="mt-2 text-xs text-muted-foreground">
            {entry.effectiveHasValue ? t("settings.effectiveSecretSet") : t("settings.effectiveSecretUnset")}
          </p>
        ) : null}
      </div>

      <div className="grid content-start gap-2">
        {field.fieldType === "bool" ? (
          <Switch
            aria-label={settingsFieldLabel(t, field)}
            isSelected={Boolean(value)}
            onChange={(selected: boolean) => onChange(selected)}
            className="flex min-h-10 w-full items-center justify-between rounded-none border px-3"
          >
            <Switch.Content>
              <span className="text-sm text-muted-foreground">
                {Boolean(value) ? t("common.enabled") : t("common.disabled")}
              </span>
            </Switch.Content>
            <Switch.Control>
              <Switch.Thumb />
            </Switch.Control>
          </Switch>
        ) : field.fieldType === "select" ? (
          <CompactSelect
            value={String(value || field.default || "")}
            options={(field.options || []).map((option) => ({
              value: option,
              label: option === "turso" ? "Turso Cloud" : option === "local" ? "Local libSQL" : option,
            }))}
            onChange={onChange}
            ariaLabel={settingsFieldLabel(t, field)}
            triggerClassName="min-h-10 text-sm"
          />
        ) : field.fieldType === "email_list" || field.fieldType === "ip_list" || field.fieldType === "url_list" ? (
          <TextArea
            id={field.key}
            value={inputValue}
            onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => onChange(event.target.value)}
            placeholder={settingsFieldPlaceholder(t, field)}
          />
        ) : (
          <Input
            id={field.key}
            type={field.fieldType === "secret" ? "password" : field.fieldType === "int" || field.fieldType === "decimal" ? "number" : field.fieldType === "url" ? "url" : field.fieldType === "email" ? "email" : "text"}
            min={field.constraints.min}
            max={field.constraints.max}
            step={field.constraints.step}
            value={inputValue}
            onChange={(event) => onChange(event.target.value)}
            placeholder={field.fieldType === "secret" && entry?.hasValue
              ? t("settings.secretKeepPlaceholder")
              : settingsFieldPlaceholder(t, field)}
          />
        )}

        {field.unit ? <span className="text-right text-xs text-muted-foreground">{field.unit}</span> : null}
        {field.fieldType === "secret" ? (
          <div className="flex min-h-8 items-center justify-between gap-3">
            <span className={`text-xs ${secretClearing ? "text-red-600" : "text-muted-foreground"}`}>
              {secretClearing
                ? t("settings.secretWillClear")
                : entry?.hasValue
                  ? t("settings.currentlySet")
                  : t("common.unset")}
            </span>
            {entry?.hasValue || secretClearing ? (
              <Button size="sm" variant="ghost" onClick={() => onChange(secretClearing ? "" : null)}>
                {secretClearing ? <RotateCcw className="h-3.5 w-3.5" /> : <AlertTriangle className="h-3.5 w-3.5" />}
                {secretClearing ? t("common.cancel") : t("settings.clearSecret")}
              </Button>
            ) : null}
          </div>
        ) : null}
        {errors.map((error) => <p key={error} className="text-xs text-red-600">{error}</p>)}
      </div>
    </div>
  );
}

function ProvisionKeyPanel({ value, error }: { value: ProvisionSshKey | null; error: string }) {
  const { t } = useLocaleText();
  return (
    <section>
      <h3 className="text-base font-semibold">{t("settings.provisionSshKeyTitle")}</h3>
      <p className="mt-2 text-sm text-muted-foreground">{t("settings.provisionSshKeyDesc")}</p>
      {error ? <Alert status="danger" className="mt-4">{error}</Alert> : null}
      {value ? (
        <div className="mt-4 grid gap-3">
          <CopyableCodeField label={t("clientMarket.publicKey")} value={value.publicKey} copyLabel={t("clientMarket.copy")} copiedLabel={t("clientMarket.copied")} />
          <CopyableCodeField label={t("clientMarket.authorizedKeysLine")} value={value.authorizedKeysLine} copyLabel={t("clientMarket.copy")} copiedLabel={t("clientMarket.copied")} />
        </div>
      ) : null}
    </section>
  );
}

function groupFields(fields: SettingsField[]) {
  const groups = new Map<string, SettingsField[]>();
  for (const field of fields) groups.set(field.group, [...(groups.get(field.group) || []), field]);
  return [...groups.entries()];
}

function groupSubsectionId(group: string) {
  return `group:${group}`;
}

function buildSettingsSubsections(
  category: SettingsCategoryId,
  fields: SettingsField[],
  values: Record<string, SettingValueEntry>,
  dirty: Record<string, DirtyValue>,
  mapDirty: boolean,
  t: ReturnType<typeof useLocaleText>["t"],
) {
  if (category === "notifications") {
    return buildNotificationSubsections(fields, values, dirty, t);
  }

  const items: SettingsSubsection[] = groupFields(fields).map(([group, groupItems]) => ({
    id: groupSubsectionId(group),
    label: settingsGroupLabel(t, group),
    groups: [group],
    fieldCount: groupItems.filter((field) => dependenciesSatisfied(field, fields, values, dirty)).length,
    dirtyCount: groupItems.filter((field) => Object.prototype.hasOwnProperty.call(dirty, field.key)).length,
  }));

  if (category === "general_display") {
    items.push(
      {
        id: PANEL_SUBSECTION_IDS.map,
        label: t("settings.map"),
        panel: "map",
        dirtyCount: mapDirty ? 1 : 0,
      },
      {
        id: PANEL_SUBSECTION_IDS.announcement,
        label: t("settings.announcement"),
        panel: "announcement",
        dirtyCount: 0,
      },
    );
  } else if (category === "marketplace") {
    items.push({
      id: PANEL_SUBSECTION_IDS.provision_key,
      label: t("settings.provisionSshKeyTitle"),
      panel: "provision_key",
      dirtyCount: 0,
    });
  }

  return items;
}

function buildNotificationSubsections(
  fields: SettingsField[],
  values: Record<string, SettingValueEntry>,
  dirty: Record<string, DirtyValue>,
  t: ReturnType<typeof useLocaleText>["t"],
) {
  const knownGroups = new Set<string>(Object.values(NOTIFICATION_SUBSECTION_GROUPS).flat());
  const subsection = (
    id: string,
    label: string,
    groups: readonly string[],
    panel?: SettingsPanel,
  ): SettingsSubsection => {
    const subsectionFields = fields.filter((field) => groups.includes(field.group));
    return {
      id,
      label,
      groups: [...groups],
      panel,
      fieldCount: subsectionFields.filter((field) => dependenciesSatisfied(field, fields, values, dirty)).length,
      dirtyCount: subsectionFields.filter((field) => Object.prototype.hasOwnProperty.call(dirty, field.key)).length,
    };
  };

  const items = [
    subsection(
      NOTIFICATION_SUBSECTION_IDS.system,
      t("settings.notifications.system"),
      NOTIFICATION_SUBSECTION_GROUPS.system,
    ),
    subsection(
      NOTIFICATION_SUBSECTION_IDS.business,
      t("settings.notifications.business"),
      NOTIFICATION_SUBSECTION_GROUPS.business,
    ),
    subsection(
      NOTIFICATION_SUBSECTION_IDS.channels,
      t("settings.notifications.channels"),
      NOTIFICATION_SUBSECTION_GROUPS.channels,
      "channel_health",
    ),
  ];

  const unclassified = fields.filter((field) => !knownGroups.has(field.group));
  items.push(...groupFields(unclassified).map(([group, groupItems]) => ({
    id: groupSubsectionId(group),
    label: settingsGroupLabel(t, group),
    groups: [group],
    fieldCount: groupItems.filter((field) => dependenciesSatisfied(field, fields, values, dirty)).length,
    dirtyCount: groupItems.filter((field) => Object.prototype.hasOwnProperty.call(dirty, field.key)).length,
  })));

  return items;
}

function subsectionIdForField(field: SettingsField) {
  if (field.category !== "notifications") return groupSubsectionId(field.group);
  for (const [subsection, groups] of Object.entries(NOTIFICATION_SUBSECTION_GROUPS)) {
    if ((groups as readonly string[]).includes(field.group)) {
      return NOTIFICATION_SUBSECTION_IDS[subsection as keyof typeof NOTIFICATION_SUBSECTION_IDS];
    }
  }
  return groupSubsectionId(field.group);
}

function categoryDirtyCount(category: SettingsCategoryId, fields: SettingsField[], dirty: Record<string, DirtyValue>) {
  return fields.filter((field) => field.category === category && Object.prototype.hasOwnProperty.call(dirty, field.key)).length;
}

function dependenciesSatisfied(
  field: SettingsField,
  fields: SettingsField[],
  values: Record<string, SettingValueEntry>,
  dirty: Record<string, DirtyValue>,
) {
  return (field.dependencies || []).every((dependency) => {
    const parent = fields.find((candidate) => candidate.key === dependency.key);
    if (!parent) return true;
    return String(dirtyValue(parent, values[parent.key], dirty)) === dependency.equals;
  });
}

function baseValue(field: SettingsField, entry?: SettingValueEntry): DirtyValue {
  const configured = entry?.value;
  if (field.fieldType === "bool") {
    const raw = configured || field.default || "";
    return raw === "true" || raw === "1" || raw === "yes" || raw === "on";
  }
  if (field.fieldType === "secret") return "";
  return configured ?? field.default ?? "";
}

function dirtyValue(field: SettingsField, entry: SettingValueEntry | undefined, dirty: Record<string, DirtyValue>): DirtyValue {
  if (Object.prototype.hasOwnProperty.call(dirty, field.key)) return dirty[field.key];
  return baseValue(field, entry);
}

function sameDirtyValue(field: SettingsField, left: DirtyValue, right: DirtyValue) {
  if (field.fieldType === "bool") return Boolean(left) === Boolean(right);
  if (field.fieldType === "secret") return left === "" && right === "";
  return String(left ?? "").trim() === String(right ?? "").trim();
}

function buildUpdates(fields: SettingsField[], dirty: Record<string, DirtyValue>) {
  const updates: Record<string, string | null> = {};
  for (const [key, value] of Object.entries(dirty)) {
    const field = fields.find((candidate) => candidate.key === key);
    if (!field) continue;
    if (field.fieldType === "bool") {
      updates[key] = Boolean(value) ? "true" : "false";
    } else if (field.fieldType === "secret") {
      if (value === null) updates[key] = null;
      else if (String(value).trim()) updates[key] = String(value).trim();
    } else {
      const trimmed = String(value ?? "").trim();
      updates[key] = trimmed === "" ? null : trimmed;
    }
  }
  return updates;
}

function reconcileDirty(snapshot: SettingsSnapshot, current: Record<string, DirtyValue>) {
  const entries = Object.fromEntries(snapshot.values.map((entry) => [entry.key, entry]));
  const next: Record<string, DirtyValue> = {};
  for (const [key, value] of Object.entries(current)) {
    const field = snapshot.schema.fields.find((candidate) => candidate.key === key);
    const entry = entries[key];
    if (!field) continue;
    if (field.fieldType === "secret") {
      if (value === null ? entry?.hasValue : String(value).trim()) next[key] = value;
      continue;
    }
    if (!sameDirtyValue(field, value, baseValue(field, entry))) next[key] = value;
  }
  return next;
}

function bannerStatus(kind: Banner["kind"]): "default" | "success" | "danger" | "warning" {
  return kind === "destructive" ? "danger" : kind;
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}
