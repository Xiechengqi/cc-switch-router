"use client";

import { Loader2, Save, Send, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, Input, ListBox, ScrollShadow, Switch, TextArea } from "@heroui/react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { VersionPanel } from "@/components/settings/version-panel";
import { getSettingsSchema, getSettingsValues, saveSettings, testTelegram, restartService } from "@/lib/api";
import type { SettingValueEntry, SettingsField, SettingsSchema } from "@/lib/types";

type DirtyValue = string | boolean | null;
const VERSION_GROUP = "__version";

export function SettingsPage() {
  const { session, loading } = useAuth();
  const { t } = useLocaleText();
  const [schema, setSchema] = React.useState<SettingsSchema | null>(null);
  const [values, setValues] = React.useState<Record<string, SettingValueEntry>>({});
  const [activeGroup, setActiveGroup] = React.useState<string>("");
  const [dirty, setDirty] = React.useState<Record<string, DirtyValue>>({});
  const [busy, setBusy] = React.useState("");
  const [banner, setBanner] = React.useState<{ kind: "default" | "success" | "destructive"; text: string } | null>(null);

  const isAdmin = !!session?.isAdmin;

  const load = React.useCallback(async () => {
    setBusy("load");
    try {
      const [nextSchema, nextValues] = await Promise.all([getSettingsSchema(), getSettingsValues()]);
      setSchema(nextSchema);
      setValues(Object.fromEntries(nextValues.values.map((entry) => [entry.key, entry])));
      setActiveGroup((current) => current || nextSchema.groups[0] || "");
      setDirty({});
      setBanner(null);
    } catch (err) {
      setBanner({ kind: "destructive", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy("");
    }
  }, []);

  React.useEffect(() => {
    if (isAdmin) load().catch(console.error);
  }, [isAdmin, load]);

  if (loading) {
    return <main className="mx-auto w-[calc(100%-2rem)] max-w-7xl py-12 text-muted-foreground">{t("common.loadingSession")}</main>;
  }

  if (!isAdmin) {
    return (
      <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-4xl gap-6 py-12 text-foreground">
        <div>
          <div className="section-label">{t("settings.title")}</div>
          <h1 className="mt-4 font-display text-4xl">{t("settings.adminRequired")}</h1>
          <p className="mt-3 text-muted-foreground">{t("settings.adminRequiredDesc")}</p>
        </div>
        <VersionPanel isAdmin={false} />
      </main>
    );
  }

  const groups = schema?.groups || [];
  const fields = activeGroup === VERSION_GROUP ? [] : (schema?.fields || []).filter((field) => field.group === activeGroup);
  const dirtyCount = Object.keys(dirty).length;

  return (
    <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-6 pb-10 text-foreground">
      <section className="flex flex-wrap justify-end gap-2">
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" onClick={() => load()} isDisabled={!!busy}>
            {busy === "load" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
            {t("common.reload")}
          </Button>
          <Button variant="outline" onClick={telegramTest} isDisabled={!!busy}>
            {busy === "telegram" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
            {t("settings.testTelegram")}
          </Button>
          <Button variant="outline" onClick={() => submit(true)} isDisabled={!!busy || dirtyCount === 0}>
            {busy === "restart" ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {t("settings.saveRestart")}
          </Button>
          <Button variant="primary" onClick={() => submit(false)} isDisabled={!!busy || dirtyCount === 0}>
            {busy === "save" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            {dirtyCount ? t("common.saveWithCount", { count: dirtyCount }) : t("common.save")}
          </Button>
        </div>
      </section>

      {banner ? <Alert status={banner.kind === "destructive" ? "danger" : banner.kind} className="!text-slate-900">{banner.text}</Alert> : null}

      <section className="grid gap-6 lg:grid-cols-[260px_1fr]">
        <Card className="h-fit rounded-lg lg:sticky lg:top-4">
          <Card.Header>
            <Card.Title>{t("settings.groups")}</Card.Title>
            <Card.Description>{t("settings.unsavedChanges", { count: dirtyCount })}</Card.Description>
          </Card.Header>
          <Card.Content>
            <ScrollShadow className="max-h-[520px]">
              <ListBox
                aria-label={t("settings.groupsAria")}
                onAction={(key: React.Key) => setActiveGroup(String(key))}
                className="gap-1 pr-3"
              >
                {groups.map((group) => {
                  const count = (schema?.fields || []).filter((field) => field.group === group && Object.prototype.hasOwnProperty.call(dirty, field.key)).length;
                  return (
                    <ListBox.Item
                      key={group}
                      id={group}
                      textValue={group}
                      className={`flex items-center justify-between ${activeGroup === group ? "bg-primary/10 text-foreground" : ""}`}
                    >
                      <span>{group}</span>
                      {count ? <Chip size="sm" variant="soft">{count}</Chip> : null}
                    </ListBox.Item>
                  );
                })}
                <ListBox.Item id={VERSION_GROUP} textValue={t("settings.version")} className={`flex items-center justify-between ${activeGroup === VERSION_GROUP ? "bg-primary/10 text-foreground" : ""}`}>
                  <span>{t("settings.version")}</span>
                </ListBox.Item>
              </ListBox>
            </ScrollShadow>
          </Card.Content>
        </Card>

        <div className="grid gap-6">
          {activeGroup === VERSION_GROUP ? (
            <VersionPanel isAdmin={true} />
          ) : (
            <Card className="rounded-lg">
              <Card.Header>
                <Card.Title>{activeGroup || t("settings.title")}</Card.Title>
                <Card.Description>{t("settings.restartFieldDesc")}</Card.Description>
              </Card.Header>
              <Card.Content className="grid gap-4">
                {busy === "load" && !schema ? <div className="text-sm text-muted-foreground">{t("settings.loading")}</div> : null}
                {fields.map((field) => (
                  <SettingsFieldRow
                    key={field.key}
                    field={field}
                    entry={values[field.key]}
                    value={dirtyValue(field, values[field.key], dirty)}
                  dirty={Object.prototype.hasOwnProperty.call(dirty, field.key)}
                  t={t}
                  onChange={(value) => setDirty((prev) => ({ ...prev, [field.key]: value }))}
                  />
                ))}
              </Card.Content>
            </Card>
          )}
        </div>
      </section>
    </main>
  );

  async function submit(thenRestart: boolean) {
    setBusy("save");
    setBanner(null);
    try {
      const updates = buildUpdates(schema, dirty);
      const result = await saveSettings(updates);
      setBanner({
        kind: "success",
        text: t("settings.saved", { updated: result.updatedKeys.length, unchanged: result.unchangedKeys.length, restartRequired: result.restartRequiredKeys.length }),
      });
      await load();
      if (thenRestart) {
        setBusy("restart");
        await restartService();
        setBanner({ kind: "default", text: t("settings.restartScheduled") });
        pollHealthAndReload().catch(console.error);
      }
    } catch (err) {
      setBanner({ kind: "destructive", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy("");
    }
  }

  async function telegramTest() {
    setBusy("telegram");
    try {
      await testTelegram();
      setBanner({ kind: "success", text: t("settings.telegramSent") });
    } catch (err) {
      setBanner({ kind: "destructive", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy("");
    }
  }
}

function SettingsFieldRow({
  field,
  entry,
  value,
  dirty,
  t,
  onChange,
}: {
  field: SettingsField;
  entry?: SettingValueEntry;
  value: DirtyValue;
  dirty: boolean;
  t: ReturnType<typeof useLocaleText>["t"];
  onChange: (value: DirtyValue) => void;
}) {
  return (
    <Card className="rounded-lg border p-0 shadow-none">
      <Card.Content className="grid gap-3 p-4 md:grid-cols-[minmax(220px,0.8fr)_minmax(0,1.2fr)]">
      <div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="font-medium" htmlFor={field.key}>{field.label}</label>
          {field.required ? <Chip size="sm" variant="soft">{t("common.required")}</Chip> : null}
          {field.restartRequired ? <Chip color="warning" size="sm" variant="soft">{t("common.restartRequired")}</Chip> : null}
          {dirty ? <Chip color="accent" size="sm" variant="soft">{t("common.changed")}</Chip> : null}
        </div>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{field.description}</p>
        <div className="mt-2 text-xs text-muted-foreground">
          <code>{field.key}</code> · {entry?.source || t("common.unset")}
          {field.fieldType === "secret" && entry?.hasValue ? ` · ${t("settings.currentlySet")}` : ""}
        </div>
      </div>
      <div className="grid content-start gap-2">
        {field.fieldType === "bool" ? (
          <div className="flex items-center gap-3 rounded-md border bg-background p-3">
            <Switch isSelected={Boolean(value)} onChange={onChange} id={field.key} />
            <span className="text-sm text-muted-foreground">{Boolean(value) ? t("common.enabled") : t("common.disabled")}</span>
          </div>
        ) : field.fieldType === "email_list" || field.fieldType === "ip_list" ? (
          <TextArea id={field.key} value={String(value ?? "")} onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => onChange(event.target.value)} placeholder={field.placeholder || ""} />
        ) : (
          <Input
            id={field.key}
            type={field.fieldType === "secret" ? "password" : field.fieldType === "int" ? "number" : field.fieldType === "url" ? "url" : field.fieldType === "email" ? "email" : "text"}
            value={String(value ?? "")}
            onChange={(event) => onChange(event.target.value)}
            placeholder={field.fieldType === "secret" && entry?.hasValue ? t("settings.secretPlaceholder") : field.placeholder || field.default || ""}
          />
        )}
      </div>
      </Card.Content>
    </Card>
  );
}

function dirtyValue(field: SettingsField, entry: SettingValueEntry | undefined, dirty: Record<string, DirtyValue>): DirtyValue {
  if (Object.prototype.hasOwnProperty.call(dirty, field.key)) return dirty[field.key];
  if (field.fieldType === "bool") {
    const raw = entry?.value || field.default || "";
    return raw === "true" || raw === "1" || raw === "yes" || raw === "on";
  }
  if (field.fieldType === "secret") return "";
  return entry?.value || "";
}

function buildUpdates(schema: SettingsSchema | null, dirty: Record<string, DirtyValue>) {
  const updates: Record<string, string | null | boolean> = {};
  for (const [key, value] of Object.entries(dirty)) {
    const field = schema?.fields.find((candidate) => candidate.key === key);
    if (!field) continue;
    if (field.fieldType === "bool") {
      updates[key] = Boolean(value);
    } else if (field.fieldType === "secret") {
      if (value === "" || value == null) continue;
      updates[key] = value === "-" ? null : String(value);
    } else {
      const trimmed = String(value ?? "").trim();
      updates[key] = trimmed === "" ? null : trimmed;
    }
  }
  return updates;
}

async function pollHealthAndReload(maxAttempts = 60) {
  for (let i = 0; i < maxAttempts; i += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    try {
      const res = await fetch("/v1/healthz", { cache: "no-store" });
      if (res.ok) {
        window.location.reload();
        return;
      }
    } catch {
      // service may be restarting
    }
  }
}
