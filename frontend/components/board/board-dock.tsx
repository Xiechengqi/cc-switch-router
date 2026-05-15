"use client";

import { MessageSquare, Send, X } from "lucide-react";
import { Button, Chip, Input, ScrollShadow, Tabs, TextArea } from "@heroui/react";
import * as React from "react";
import { getBoardMessages, getBoardMeta, postBoardMessage, setBoardFeature, setBoardPin, deleteBoardMessage } from "@/lib/api";
import type { BoardMessage, BoardMeta } from "@/lib/types";
import { useAuth } from "@/components/auth/auth-provider";
import { formatRelativeTime } from "@/lib/utils";

const DOCK_KEY = "cc_switch_router_board_dock_v1";
const GUEST_NAME_KEY = "cc_switch_router_board_guest_name_v1";

export function BoardDock() {
  const { session } = useAuth();
  const dockRef = React.useRef<HTMLElement | null>(null);
  const [open, setOpen] = React.useState(true);
  const [tab, setTab] = React.useState("all");
  const [meta, setMeta] = React.useState<BoardMeta | null>(null);
  const [messages, setMessages] = React.useState<BoardMessage[]>([]);
  const [body, setBody] = React.useState("");
  const [guestName, setGuestName] = React.useState("");
  const [status, setStatus] = React.useState("");
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    setOpen(localStorage.getItem(DOCK_KEY) !== "closed");
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
    setOpen(next);
    localStorage.setItem(DOCK_KEY, next ? "open" : "closed");
  }

  React.useEffect(() => {
    if (!open) return;
    function handlePointerDown(event: PointerEvent) {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (dockRef.current?.contains(target)) return;
      setDockOpen(false);
    }
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [open]);

  async function send() {
    const trimmed = body.trim();
    if (!trimmed) return;
    if (trimmed.length > (meta?.maxBodyLength || 1000)) {
      setStatus(`Over ${meta?.maxBodyLength || 1000} characters`);
      return;
    }
    setBusy(true);
    setStatus("");
    try {
      if (!session?.authenticated && guestName.trim()) localStorage.setItem(GUEST_NAME_KEY, guestName.trim());
      await postBoardMessage(trimmed, session?.authenticated ? undefined : guestName.trim());
      setBody("");
      setStatus("Sent");
      await load();
      window.setTimeout(() => setStatus(""), 1600);
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  if (!open) {
    return (
      <Button className="fixed bottom-5 right-5 z-40 rounded-full shadow-lg" isIconOnly onClick={() => setDockOpen(true)} aria-label="Open message board">
        <MessageSquare className="h-5 w-5" />
      </Button>
    );
  }

  return (
    <aside ref={dockRef} className="fixed bottom-5 right-5 z-40 flex h-[min(620px,calc(100vh-2rem))] w-[min(420px,calc(100vw-2rem))] flex-col rounded-lg border bg-card shadow-2xl">
      <div className="flex items-center justify-between gap-3 border-b p-4">
        <div>
          <div className="font-semibold">Message Board</div>
          <div className="text-xs text-muted-foreground">{messages.length} visible messages</div>
        </div>
        <Button variant="ghost" isIconOnly onClick={() => setDockOpen(false)} aria-label="Close message board">
          <X className="h-4 w-4" />
        </Button>
      </div>
      <div className="border-b p-3">
        <Tabs selectedKey={tab} onSelectionChange={(key) => setTab(String(key))} variant="secondary" className="text-foreground">
          <Tabs.List className="grid w-full grid-cols-3 text-foreground">
            <Tabs.Tab id="all" className="text-muted-foreground data-[selected=true]:text-foreground">All</Tabs.Tab>
            <Tabs.Tab id="pinned" className="text-muted-foreground data-[selected=true]:text-foreground">Pinned</Tabs.Tab>
            <Tabs.Tab id="featured" className="text-muted-foreground data-[selected=true]:text-foreground">Featured</Tabs.Tab>
          </Tabs.List>
        </Tabs>
      </div>
      <ScrollShadow className="min-h-0 flex-1 p-4">
        <div className="grid gap-3 pr-3">
          {messages.length ? (
            messages.map((message) => (
              <article key={message.id} className="rounded-lg border bg-background p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{message.authorLabel || "Guest"}</span>
                  {message.pinned ? <Chip color="warning" size="sm" variant="soft">Pinned</Chip> : null}
                  {message.featured && !message.pinned ? <Chip size="sm" variant="soft">Featured</Chip> : null}
                  <span className="ml-auto text-xs text-muted-foreground">{formatRelativeTime(message.createdAt)}</span>
                </div>
                <p className="mt-2 whitespace-pre-wrap break-words text-sm leading-6">{message.body}</p>
                {meta?.canPostAsAdmin || (message.isMine && message.authorKind === "guest") ? (
                  <div className="mt-3 flex flex-wrap gap-2">
                    {meta?.canPostAsAdmin ? (
                      <>
                        <Button variant="outline" size="sm" onClick={() => setBoardPin(message.id, !message.pinned).then(load).catch(console.error)}>
                          {message.pinned ? "Unpin" : "Pin"}
                        </Button>
                        <Button variant="outline" size="sm" onClick={() => setBoardFeature(message.id, !message.featured).then(load).catch(console.error)}>
                          {message.featured ? "Unfeature" : "Feature"}
                        </Button>
                      </>
                    ) : null}
                    <Button variant="ghost" size="sm" className="text-destructive" onClick={() => deleteBoardMessage(message.id).then(load).catch(console.error)}>
                      Delete
                    </Button>
                  </div>
                ) : null}
              </article>
            ))
          ) : (
            <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">No messages yet.</div>
          )}
        </div>
      </ScrollShadow>
      <div className="grid gap-3 border-t p-4">
        {!session?.authenticated ? (
          <Input value={guestName} onChange={(event) => setGuestName(event.target.value)} placeholder="Guest name" />
        ) : null}
        <TextArea value={body} onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setBody(event.target.value)} placeholder="Write a message" maxLength={meta?.maxBodyLength || 1000} />
        <div className="flex items-center justify-between gap-3">
          <span className="text-xs text-muted-foreground">
            {status || `${body.length}/${meta?.maxBodyLength || 1000}`}
          </span>
          <Button onClick={send} isDisabled={busy || !body.trim()} size="sm">
            <Send className="h-4 w-4" />
            Send
          </Button>
        </div>
      </div>
    </aside>
  );
}
