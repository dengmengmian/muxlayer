import { useEffect, useRef, useState, useCallback } from "react";
import {
  MessageCircle,
  Send,
  Trash2,
  Loader2,
  Brain,
  Plus,
  X,
} from "lucide-react";
import { events } from "@/lib/bindings";
import {
  getPetSettings,
  getPetChatHistory,
  savePetChatHistory,
  getPetMemory,
  savePetMemory,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { toast } from "@/components/common/Toast";
import { EmptyState } from "@/components/common/EmptyState";
import type { PetType } from "@/types/pet";
import { PET_COMPONENTS } from "@/pet/petComponents";
import { sendPetMessage, parseHistory, type ChatMessage } from "@/pet/chatCore";
import {
  visibleEntries,
  mergeMemory,
  memoryLabel,
  type MemoryEntry,
} from "@/pet/petMemory";

function parseMemory(raw: string): Record<string, string> {
  try {
    const o = JSON.parse(raw);
    return o && typeof o === "object" ? o : {};
  } catch {
    return {};
  }
}

/// 主窗口的宠物聊天页。展示与桌宠共享的聊天记录(DB 持久化),
/// 支持直接发消息——和双击桌宠走同一条 pet_chat 链路、同一份记忆和人格。
/// 任一窗口发消息都会广播 PetChatUpdated,这里 listen 后实时刷新。
export function PetChat() {
  const { t, locale } = useI18n();
  const [petType, setPetType] = useState<PetType>("robot");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  // 记忆:memoryRef 是传给聊天核心的规范对象(会被 chatCore 原地更新),
  // memory state 只为渲染。两者由 mount / petMemoryChanged / 编辑保存三处同步。
  const memoryRef = useRef<Record<string, string>>({});
  const [memory, setMemory] = useState<Record<string, string>>({});
  const [memoryOpen, setMemoryOpen] = useState(false);
  const [draft, setDraft] = useState<MemoryEntry[]>([]);

  const PetAvatar = PET_COMPONENTS[petType];
  const petName = t(`settings.pet.${petType}`);

  useEffect(() => {
    getPetSettings()
      .then((s) => setPetType(s.pet_type as PetType))
      .catch(() => {});
    getPetChatHistory()
      .then((raw) => setMessages(parseHistory(raw)))
      .catch(() => {});
    getPetMemory()
      .then((raw) => {
        const m = parseMemory(raw);
        memoryRef.current = m;
        setMemory(m);
      })
      .catch(() => {});

    const unType = events.petSettingsChanged.listen((e) =>
      setPetType(e.payload.pet_type as PetType)
    );
    // 桌宠发消息 / 清空 → 这里实时同步
    const unChat = events.petChatUpdated.listen((e) =>
      setMessages(parseHistory(e.payload))
    );
    // 桌宠聊天自动提取 / 清空记忆 → 同步记忆(编辑面板打开时不覆盖草稿)
    const unMem = events.petMemoryChanged.listen((e) => {
      const m = parseMemory(e.payload);
      memoryRef.current = m;
      setMemory(m);
    });
    return () => {
      unType.then((fn) => fn());
      unChat.then((fn) => fn());
      unMem.then((fn) => fn());
    };
  }, []);

  // 新消息滚到底
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [messages, sending]);

  const handleSend = useCallback(async () => {
    const msg = input.trim();
    if (!msg || sending) return;
    setInput("");
    setSending(true);
    // 乐观显示用户消息(落库后的 petChatUpdated 会用 DB 真值覆盖,幂等)
    setMessages((prev) => [
      ...prev,
      { role: "user", content: msg, ts: Date.now() },
    ]);
    const result = await sendPetMessage({
      userMsg: msg,
      petType,
      locale,
      history: messages,
      memory: memoryRef.current,
    });
    setSending(false);
    if (!result.ok) {
      // 错误不进历史,单独提示在列表尾
      setMessages((prev) => [
        ...prev,
        {
          role: "assistant",
          content: `⚠️ ${result.errorText}`,
          ts: Date.now(),
        },
      ]);
    }
  }, [input, sending, petType, locale, messages]);

  const handleClear = useCallback(() => {
    if (!confirmClear) {
      setConfirmClear(true);
      setTimeout(() => setConfirmClear(false), 3000);
      return;
    }
    setConfirmClear(false);
    savePetChatHistory("[]").catch(() => {});
    setMessages([]);
  }, [confirmClear]);

  // ── 记忆编辑 ──
  const toggleMemory = useCallback(() => {
    setMemoryOpen((open) => {
      // 打开时用当前记忆初始化草稿(打开期间不被事件覆盖)
      if (!open) setDraft(visibleEntries(memory));
      return !open;
    });
  }, [memory]);

  const updateDraft = useCallback((i: number, patch: Partial<MemoryEntry>) => {
    setDraft((d) => d.map((e, idx) => (idx === i ? { ...e, ...patch } : e)));
  }, []);

  const removeDraft = useCallback((i: number) => {
    setDraft((d) => d.filter((_, idx) => idx !== i));
  }, []);

  const addDraft = useCallback(() => {
    setDraft((d) => [...d, { key: "", value: "" }]);
  }, []);

  const saveMemory = useCallback(() => {
    const next = mergeMemory(memoryRef.current, draft);
    memoryRef.current = next;
    setMemory(next);
    setMemoryOpen(false);
    savePetMemory(JSON.stringify(next)).catch(() => {});
    toast("success", t("petchat.memory_saved"));
  }, [draft, t]);

  const memoryCount = visibleEntries(memory).length;

  return (
    <div
      data-testid="pet-chat-page"
      className="desktop-page h-full min-h-0 overflow-hidden"
    >
      <header className="desktop-page-header">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 className="flex items-center gap-2 text-lg font-semibold text-text-primary">
              <MessageCircle className="h-5 w-5" />
              {t("petchat.title")}
            </h2>
            <p className="mt-1 max-w-2xl text-sm text-text-muted">
              {t("petchat.desc")}
            </p>
          </div>
          {messages.length > 0 && (
            <button
              onClick={handleClear}
              className="flex items-center gap-1.5 rounded-lg border border-border bg-card-secondary px-3 py-1.5 text-sm text-text-muted transition-colors hover:border-error/40 hover:text-error"
            >
              <Trash2 className="h-4 w-4" />
              {confirmClear ? t("petchat.clear_confirm") : t("petchat.clear")}
            </button>
          )}
        </div>
      </header>

      <section className="surface-panel flex min-h-0 flex-1 flex-col overflow-hidden">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div>
            <h3 className="text-sm font-semibold text-text-primary">
              {t("petchat.conversation_stream")}
            </h3>
            <p className="mt-0.5 text-xs text-text-muted">
              {t("petchat.conversation_stream_hint")}
            </p>
          </div>
          <span className="rounded-full bg-card-secondary px-2 py-0.5 font-mono text-xs text-text-muted">
            {messages.length}
          </span>
        </div>
        <div
          ref={scrollRef}
          className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4"
        >
          {messages.length === 0 ? (
            <EmptyState
              icon={MessageCircle}
              title={petName}
              description={t("petchat.empty")}
            />
          ) : (
            messages.map((m, i) => (
              <div
                key={i}
                className={
                  m.role === "user" ? "flex justify-end" : "flex gap-2"
                }
              >
                {m.role === "assistant" && (
                  <div className="mt-0.5 h-8 w-8 shrink-0 overflow-hidden">
                    <div className="flex h-full w-full items-end justify-center [&>svg]:h-full [&>svg]:w-auto">
                      <PetAvatar state="idle" />
                    </div>
                  </div>
                )}
                <div
                  className={
                    m.role === "user"
                      ? "max-w-[75%] rounded-2xl rounded-br-sm bg-accent px-3 py-2 text-sm text-on-accent"
                      : "max-w-[75%] rounded-2xl rounded-bl-sm border border-border bg-bg px-3 py-2 text-sm text-text-primary"
                  }
                >
                  {m.role === "assistant" && (
                    <div className="mb-0.5 text-xs font-medium text-text-muted">
                      {petName}
                    </div>
                  )}
                  <div className="whitespace-pre-wrap break-words">
                    {m.content}
                  </div>
                </div>
              </div>
            ))
          )}
          {sending && (
            <div className="flex items-center gap-2 text-sm text-text-muted">
              <Loader2 className="h-4 w-4 animate-spin" />
              {petName}…
            </div>
          )}
        </div>

        <div
          data-testid="pet-memory-panel"
          className={`max-h-56 space-y-2 overflow-y-auto border-t border-border bg-card-secondary/35 px-4 py-3 ${
            memoryOpen ? "" : "hidden"
          }`}
        >
          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
                <Brain className="h-4 w-4" />
                {t("petchat.memory_matrix")}
                <span className="text-text-muted">({memoryCount})</span>
              </div>
              <p className="mt-0.5 text-xs text-text-muted">
                {t("petchat.memory_desc")}
              </p>
            </div>
            <button
              onClick={() => setMemoryOpen(false)}
              className="rounded-lg p-1.5 text-text-muted transition-colors hover:bg-bg hover:text-text-primary"
              aria-label="close"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          {draft.length === 0 && (
            <p className="py-1 text-sm text-text-muted">
              {t("petchat.memory_empty")}
            </p>
          )}
          {draft.map((e, i) => (
            <div key={i} className="flex items-center gap-2">
              <input
                value={e.key}
                onChange={(ev) => updateDraft(i, { key: ev.target.value })}
                placeholder={t("petchat.memory_key")}
                list="petchat-memory-keys"
                className="w-32 shrink-0 rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm text-text-primary outline-none focus:border-accent"
              />
              <input
                value={e.value}
                onChange={(ev) => updateDraft(i, { value: ev.target.value })}
                placeholder={memoryLabel(e.key, locale)}
                className="flex-1 rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm text-text-primary outline-none focus:border-accent"
              />
              <button
                onClick={() => removeDraft(i)}
                className="rounded-lg p-1.5 text-text-muted transition-colors hover:bg-error-soft hover:text-error"
                aria-label="remove"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
          ))}
          <datalist id="petchat-memory-keys">
            <option value="name" />
            <option value="topic" />
          </datalist>
          <div className="flex items-center justify-between pt-1">
            <button
              onClick={addDraft}
              className="flex items-center gap-1 text-sm text-text-muted transition-colors hover:text-accent"
            >
              <Plus className="h-4 w-4" />
              {t("petchat.memory_add")}
            </button>
            <button
              onClick={saveMemory}
              className="rounded-lg bg-accent px-3 py-1.5 text-sm font-medium text-on-accent"
            >
              {t("petchat.memory_save")}
            </button>
          </div>
        </div>

        <div className="flex items-center gap-2 border-t border-border p-3">
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) handleSend();
            }}
            placeholder={t("petchat.placeholder")}
            className="flex-1 rounded-lg border border-border bg-bg px-3 py-2 text-sm text-text-primary outline-none focus:border-accent"
          />
          <button
            type="button"
            onClick={toggleMemory}
            className={`flex items-center gap-1.5 rounded-lg border px-3 py-2 text-sm font-medium transition-colors ${
              memoryOpen
                ? "border-accent/40 bg-accent-soft text-accent"
                : "border-border bg-bg text-text-muted hover:border-accent/40 hover:text-accent"
            }`}
          >
            <Brain className="h-4 w-4" />
            {t("petchat.memory_matrix")}
            <span className="font-mono text-xs opacity-70">{memoryCount}</span>
          </button>
          <button
            onClick={handleSend}
            disabled={!input.trim() || sending}
            className="flex items-center gap-1.5 rounded-lg bg-accent px-4 py-2 text-sm font-medium text-on-accent transition-opacity disabled:opacity-40"
          >
            <Send className="h-4 w-4" />
            {t("petchat.send")}
          </button>
        </div>
      </section>
    </div>
  );
}
