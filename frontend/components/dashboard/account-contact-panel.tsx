"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Loader2, Plus, Save, Trash2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountContacts, updateAccountProviderContacts } from "@/lib/api";
import type { ProviderContact, ProviderContactChannel } from "@/lib/types";

const CHANNELS: ProviderContactChannel[] = ["wechat", "telegram", "custom"];

export function AccountContactPanel() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const actor = session?.authenticated
    ? session.user?.id || session.user?.email || "authenticated"
    : "anonymous";
  const [snapshot, setSnapshot] = React.useState<{
    actor: string;
    contacts: ProviderContact[];
    saved: string;
  } | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [error, setError] = React.useState("");
  const [attempt, setAttempt] = React.useState(0);

  React.useEffect(() => {
    if (!session?.authenticated) return;
    let active = true;
    setSnapshot(null);
    setLoading(true);
    setError("");
    void getAccountContacts().then((profile) => {
      if (active) setSnapshot({
        actor,
        contacts: profile.contacts || [],
        saved: JSON.stringify(profile.contacts || []),
      });
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [actor, attempt, session?.authenticated]);

  if (authLoading || loading || (session?.authenticated && (!snapshot || snapshot.actor !== actor) && !error)) return <p className="flex items-center gap-2 py-6 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</p>;
  if (!session?.authenticated) return <p className="py-6 text-sm text-muted-foreground">{t("account.signInRequired")}</p>;
  if (!snapshot || snapshot.actor !== actor) return <div className="grid justify-items-start gap-3 py-6"><p role="alert" className="text-sm text-destructive">{error || t("common.loading")}</p><Button variant="outline" onPress={() => setAttempt((value) => value + 1)}>{t("common.retry")}</Button></div>;

  const contacts = snapshot.contacts;
  const saved = snapshot.saved;
  const update = (next: ProviderContact[]) => setSnapshot((current) => current?.actor === actor
    ? { ...current, contacts: next } : current);
  const save = async () => {
    if (!snapshot || snapshot.actor !== actor || saving) return;
    setSaving(true);
    setError("");
    try {
      const profile = await updateAccountProviderContacts(contacts);
      setSnapshot({ actor, contacts: profile.contacts || [], saved: JSON.stringify(profile.contacts || []) });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  };

  return <section className="grid gap-5 rounded-xl border border-border bg-card p-5">
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 className="text-base font-semibold">{t("account.contact.title")}</h2>
        <p className="mt-1 text-sm text-muted-foreground">{t("account.contact.hint")}</p>
      </div>
      <Button variant="primary" isDisabled={saving || JSON.stringify(contacts) === saved} onPress={() => void save()}>
        {saving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}{t("common.save")}
      </Button>
    </div>
    {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
    {contacts.map((contact, index) => <div key={index} className="grid gap-2 sm:grid-cols-[10rem_minmax(0,1fr)_2.75rem] sm:items-end">
      <label className="grid gap-1 text-sm">
        <span>{t("account.contact.channel")}</span>
        <select className="h-11 rounded-md border border-border bg-background px-3" value={contact.channel} onChange={(event) => update(contacts.map((item, i) => i === index ? { ...item, channel: event.target.value as ProviderContactChannel } : item))}>
          {CHANNELS.map((channel) => <option key={channel} value={channel}>{t(`account.contact.channel.${channel}`)}</option>)}
        </select>
      </label>
      <label className="grid gap-1 text-sm">
        <span>{t("account.contact.handle")}</span>
        <input className="h-11 min-w-0 rounded-md border border-border bg-background px-3" value={contact.handle} placeholder={t("account.contact.handlePlaceholder")} onChange={(event) => update(contacts.map((item, i) => i === index ? { ...item, handle: event.target.value } : item))} />
      </label>
      <Button isIconOnly variant="ghost" aria-label={t("account.contact.remove")} onPress={() => update(contacts.filter((_, i) => i !== index))}><Trash2 className="h-4 w-4" /></Button>
    </div>)}
    {!contacts.length ? <p className="text-sm text-muted-foreground">{t("account.contact.emptyEditor")}</p> : null}
    <Button variant="outline" className="justify-self-start" isDisabled={contacts.length >= 10} onPress={() => update([...contacts, { channel: "wechat", handle: "" }])}><Plus className="h-4 w-4" />{t("account.contact.add")}</Button>
  </section>;
}
