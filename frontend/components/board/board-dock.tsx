"use client";

import { MessageSquare, Send, X } from "lucide-react";
import { Button, Card, Chip, Input, ScrollShadow, Tabs, TextArea } from "@heroui/react";
import * as React from "react";
import { getBoardMessages, getBoardMeta, postBoardMessage, setBoardFeature, setBoardPin, deleteBoardMessage } from "@/lib/api";
import type { BoardMessage, BoardMeta } from "@/lib/types";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatRelativeTime } from "@/lib/utils";

const GUEST_NAME_KEY = "cc_switch_router_board_guest_name_v1";

export function BoardDock() {
  const { session } = useAuth();
  const { t } = useLocaleText();
  const dockRef = React.useRef<HTMLDivElement | null>(null);
  const [dockMode, setDockMode] = React.useState<"closed" | "hover" | "pinned">("closed");
  const [tab, setTab] = React.useState("all");
  const [meta, setMeta] = React.useState<BoardMeta | null>(null);
  const [messages, setMessages] = React.useState<BoardMessage[]>([]);
  const [body, setBody] = React.useState("");
  const [guestName, setGuestName] = React.useState("");
  const [status, setStatus] = React.useState("");
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    setGuestName(localStorage.getItem(GUEST_NAME_KEY) || "");
  }, []);

  const load = React.useCallback(async () => {
    const [nextMeta, list] = await Promise.all([getBoardMeta(), getBoardMessages(tab)]);
    setMeta(nextMeta);
    setMessages(list.messages || []);
  }, [tab]);

  React.useEffect(() => {
    load().catch(console.error);
    const id = window.setInterval(() => load().catch(console.error), 7000);
    return () => window.clearInterval(id);
  }, [load]);

  function setDockOpen(next: boolean) {
    setDockMode(next ? "pinned" : "closed");
  }

  React.useEffect(() => {
    if (dockMode !== "pinned") return;
    function handlePointerDown(event: PointerEvent) {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (dockRef.current?.contains(target)) return;
      setDockOpen(false);
    }
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [dockMode]);

  async function send() {
    const trimmed = body.trim();
    if (!trimmed) return;
    if (trimmed.length > (meta?.maxBodyLength || 1000)) {
      setStatus(t("board.overLimit", { count: meta?.maxBodyLength || 1000 }));
      return;
    }
    setBusy(true);
    setStatus("");
    try {
      if (!session?.authenticated && guestName.trim()) localStorage.setItem(GUEST_NAME_KEY, guestName.trim());
      await postBoardMessage(trimmed, session?.authenticated ? undefined : guestName.trim());
      setBody("");
      setStatus(t("board.sent"));
      await load();
      window.setTimeout(() => setStatus(""), 1600);
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (dockMode === "closed") {
    return (
      <Button
        className="fixed bottom-5 right-5 z-40 rounded-full shadow-lg"
        isIconOnly
        onClick={() => setDockMode("pinned")}
        onMouseEnter={() => setDockMode("hover")}
        aria-label={t("board.open")}
      >
        <MessageSquare className="h-5 w-5" />
      </Button>
    );
  }

  return (
    <Card
      ref={dockRef}
      className="fixed bottom-5 right-5 z-40 flex h-[min(620px,calc(100vh-2rem))] w-[min(420px,calc(100vw-2rem))] flex-col gap-0 overflow-hidden rounded-lg border bg-card p-0 shadow-2xl"
      onClick={() => {
        if (dockMode === "hover") setDockMode("pinned");
      }}
      onMouseLeave={() => {
        if (dockMode === "hover") setDockMode("closed");
      }}
    >
      <Card.Header className="flex-row items-center justify-between gap-3 border-b p-4">
        <div>
          <Card.Title>{t("board.title")}</Card.Title>
          <Card.Description className="!text-slate-500">{t("board.visibleMessages", { count: messages.length })}</Card.Description>
        </div>
        <Button
          variant="ghost"
          isIconOnly
          onClick={(event) => {
            event.stopPropagation();
            setDockOpen(false);
          }}
          aria-label={t("board.close")}
        >
          <X className="h-4 w-4" />
        </Button>
      </Card.Header>
      <Card.Content className="min-h-0 gap-0 p-0">
        <div className="border-b p-3">
          <Tabs selectedKey={tab} onSelectionChange={(key) => setTab(String(key))} variant="secondary" className="text-foreground">
            <Tabs.List className="grid w-full grid-cols-3 text-foreground">
              <Tabs.Tab id="all" className="text-muted-foreground data-[selected=true]:text-foreground">{t("board.all")}</Tabs.Tab>
              <Tabs.Tab id="pinned" className="text-muted-foreground data-[selected=true]:text-foreground">{t("board.pinned")}</Tabs.Tab>
              <Tabs.Tab id="featured" className="text-muted-foreground data-[selected=true]:text-foreground">{t("board.featured")}</Tabs.Tab>
            </Tabs.List>
          </Tabs>
        </div>
        <ScrollShadow className="min-h-0 flex-1 p-4">
          <div className="grid gap-3 pr-3">
            {messages.length ? (
              messages.map((message) => (
                <Card key={message.id} className="rounded-lg border bg-background p-0 shadow-none">
                  <Card.Content className="p-3">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{message.authorLabel || t("board.guest")}</span>
                      {message.pinned ? <Chip color="warning" size="sm" variant="soft">{t("board.pinned")}</Chip> : null}
                      {message.featured && !message.pinned ? <Chip size="sm" variant="soft">{t("board.featured")}</Chip> : null}
                      <span className="ml-auto text-xs text-muted-foreground">{formatRelativeTime(message.createdAt)}</span>
                    </div>
                    <p className="mt-2 whitespace-pre-wrap break-words text-sm leading-6">{message.body}</p>
                    {meta?.canPostAsAdmin || (message.isMine && message.authorKind === "guest") ? (
                      <div className="mt-3 flex flex-wrap gap-2">
                        {meta?.canPostAsAdmin ? (
                          <>
                            <Button variant="outline" size="sm" onClick={() => setBoardPin(message.id, !message.pinned).then(load).catch(console.error)}>
                              {message.pinned ? t("board.unpin") : t("board.pin")}
                            </Button>
                            <Button variant="outline" size="sm" onClick={() => setBoardFeature(message.id, !message.featured).then(load).catch(console.error)}>
                              {message.featured ? t("board.unfeature") : t("board.feature")}
                            </Button>
                          </>
                        ) : null}
                        <Button variant="ghost" size="sm" className="text-destructive" onClick={() => deleteBoardMessage(message.id).then(load).catch(console.error)}>
                          {t("common.delete")}
                        </Button>
                      </div>
                    ) : null}
                  </Card.Content>
                </Card>
              ))
            ) : (
              <Card className="rounded-lg border border-dashed p-0 text-center shadow-none">
                <Card.Content className="p-6 text-sm text-muted-foreground">{t("board.empty")}</Card.Content>
              </Card>
            )}
          </div>
        </ScrollShadow>
        <div className="grid gap-3 border-t p-4">
          {!session?.authenticated ? (
            <Input value={guestName} onChange={(event) => setGuestName(event.target.value)} placeholder={t("board.guestName")} />
          ) : null}
          <TextArea value={body} onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setBody(event.target.value)} placeholder={t("board.write")} maxLength={meta?.maxBodyLength || 1000} />
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs text-muted-foreground">
              {status || `${body.length}/${meta?.maxBodyLength || 1000}`}
            </span>
            <Button onClick={send} isDisabled={busy || !body.trim()} size="sm">
              <Send className="h-4 w-4" />
              {t("board.send")}
            </Button>
          </div>
        </div>
      </Card.Content>
    </Card>
  );
}
