import { useId, type ReactNode } from "react";

// Original identification marks. Colors follow each product; geometry is
// ours so we don't ship official trademarks.

export const CLIENT_LOGO_IDS = [
  "codex",
  "claude_code",
  "opencode",
  "gemini_cli",
  "atomcode",
  "claude_desktop",
  "kimi_cli",
  "grok_build",
  "deepseek_harness",
  "codeleveler",
] as const;

export type ClientLogoId = (typeof CLIENT_LOGO_IDS)[number];

const LABELS: Record<ClientLogoId, string> = {
  codex: "Codex",
  claude_code: "Claude Code",
  opencode: "OpenCode",
  gemini_cli: "Gemini CLI",
  atomcode: "AtomCode",
  claude_desktop: "Claude Desktop",
  kimi_cli: "Kimi CLI",
  grok_build: "Grok Build",
  deepseek_harness: "DeepSeek Harness",
  codeleveler: "CodeLeveler",
};

function Tile({ fill, children }: { fill: string; children: ReactNode }) {
  return (
    <>
      <rect
        width="24"
        height="24"
        rx="5"
        fill={fill}
        stroke="rgba(255,255,255,0.14)"
        strokeWidth="1"
      />
      {children}
    </>
  );
}

function CodexMark() {
  return (
    <Tile fill="#111111">
      <path
        d="M7.2 8.4 11.4 12 7.2 15.6"
        fill="none"
        stroke="#F4F4F5"
        strokeWidth="1.9"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M12.4 15.6h4.6"
        fill="none"
        stroke="#F4F4F5"
        strokeWidth="1.9"
        strokeLinecap="round"
      />
    </Tile>
  );
}

function ClaudeStar({ fill = "#F3E6DC" }: { fill?: string }) {
  return (
    <path
      fill={fill}
      d="M12 4.2c.45 2.6 1.7 4.6 4.2 5.8-2.5 1.2-3.75 3.2-4.2 5.8-.45-2.6-1.7-4.6-4.2-5.8 2.5-1.2 3.75-3.2 4.2-5.8Z"
    />
  );
}

function ClaudeCodeMark() {
  return (
    <Tile fill="#C15F3C">
      <ClaudeStar />
    </Tile>
  );
}

function OpenCodeMark() {
  return (
    <Tile fill="#F4F1EA">
      <rect x="5" y="4.5" width="14" height="15" rx="2" fill="#171717" />
      <rect x="7" y="7" width="10" height="10" rx="1" fill="#F4F1EA" />
      <path
        d="M9.2 10.2 8 12l1.2 1.8M14.8 10.2 16 12l-1.2 1.8"
        stroke="#171717"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
      />
    </Tile>
  );
}

function GeminiMark({ uid }: { uid: string }) {
  return (
    <Tile fill="#0E1424">
      <defs>
        <linearGradient id={uid} x1="4" y1="4" x2="20" y2="20">
          <stop offset="0%" stopColor="#4C8DFF" />
          <stop offset="45%" stopColor="#9B72F5" />
          <stop offset="100%" stopColor="#F06292" />
        </linearGradient>
      </defs>
      <path
        fill={`url(#${uid})`}
        d="M12 3.2c.7 3.4 2.4 5.8 5.8 6.8-3.4.9-5.1 3.4-5.8 6.8-.7-3.4-2.4-5.9-5.8-6.8 3.4-1 5.1-3.4 5.8-6.8Z"
      />
    </Tile>
  );
}

function AtomCodeMark() {
  return (
    <Tile fill="#163028">
      <ellipse
        cx="12"
        cy="12"
        rx="8"
        ry="3.2"
        fill="none"
        stroke="#7DDB9A"
        strokeWidth="1.4"
      />
      <ellipse
        cx="12"
        cy="12"
        rx="8"
        ry="3.2"
        fill="none"
        stroke="#7DDB9A"
        strokeWidth="1.4"
        transform="rotate(60 12 12)"
      />
      <ellipse
        cx="12"
        cy="12"
        rx="8"
        ry="3.2"
        fill="none"
        stroke="#7DDB9A"
        strokeWidth="1.4"
        transform="rotate(-60 12 12)"
      />
      <circle cx="12" cy="12" r="2" fill="#E8FFF0" />
    </Tile>
  );
}

function ClaudeDesktopMark() {
  return (
    <Tile fill="#2A211C">
      <rect x="4.5" y="6" width="15" height="12" rx="2" fill="#C15F3C" />
      <rect x="5.5" y="8.2" width="13" height="8.3" rx="1" fill="#2A211C" />
      <g transform="translate(0 1.2) scale(0.78) translate(3.4 2.4)">
        <ClaudeStar fill="#F3E6DC" />
      </g>
    </Tile>
  );
}

function KimiMark() {
  return (
    <Tile fill="#111111">
      <circle cx="13" cy="12" r="6.2" fill="#F5F5F5" />
      <circle cx="16.4" cy="10.6" r="5.2" fill="#111111" />
    </Tile>
  );
}

function GrokMark() {
  return (
    <Tile fill="#000000">
      <circle
        cx="12"
        cy="12"
        r="7.2"
        fill="none"
        stroke="#F5F5F5"
        strokeWidth="1.6"
      />
      <circle
        cx="12"
        cy="12"
        r="3.6"
        fill="none"
        stroke="#F5F5F5"
        strokeWidth="1.4"
      />
      <circle cx="13.4" cy="11" r="1.5" fill="#F5F5F5" />
    </Tile>
  );
}

function CodelevelerMark() {
  return (
    <Tile fill="#143D36">
      <path
        d="M7 6.5h6.2c2.4 0 3.8 1.3 3.8 3.2 0 1.4-.8 2.5-2.1 3l2.4 4.8h-2.5l-2.1-4.4H9.2V17.5H7V6.5Zm2.2 1.8v3.4h3.6c1.1 0 1.8-.5 1.8-1.7s-.7-1.7-1.8-1.7H9.2Z"
        fill="#E7F6F2"
      />
    </Tile>
  );
}

function DeepSeekMark() {
  return (
    <Tile fill="#4D6BFE">
      <path
        fill="#FFFFFF"
        d="M5.5 13.2c1.2-3.4 4.4-5.8 8.3-5.8 1.6 0 3 .4 4.1 1.1 0-1.3.7-2.4 1.9-2.9-.2 1.3.2 2.5 1.1 3.4-1.2 3.2-4.3 5.4-8.1 5.4-1.3 0-2.5-.3-3.6-.7L6.8 17.4 5.5 13.2Z"
      />
      <circle cx="14.6" cy="10.6" r="0.85" fill="#4D6BFE" />
    </Tile>
  );
}

function Mark({ id, uid }: { id: ClientLogoId; uid: string }) {
  switch (id) {
    case "codex":
      return <CodexMark />;
    case "claude_code":
      return <ClaudeCodeMark />;
    case "opencode":
      return <OpenCodeMark />;
    case "gemini_cli":
      return <GeminiMark uid={uid} />;
    case "atomcode":
      return <AtomCodeMark />;
    case "claude_desktop":
      return <ClaudeDesktopMark />;
    case "kimi_cli":
      return <KimiMark />;
    case "grok_build":
      return <GrokMark />;
    case "deepseek_harness":
      return <DeepSeekMark />;
    case "codeleveler":
      return <CodelevelerMark />;
  }
}

export function ClientLogo({
  id,
  className,
}: {
  id: ClientLogoId;
  className?: string;
}) {
  const uid = useId().replace(/:/g, "");
  return (
    <svg
      viewBox="0 0 24 24"
      className={className}
      role="img"
      aria-label={LABELS[id]}
      data-testid={`client-logo-${id}`}
    >
      <Mark id={id} uid={`${uid}-g`} />
    </svg>
  );
}
