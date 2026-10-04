import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { ConnectionPanel } from "./ConnectionPanel";

const meta = {
  title: "Windows/Connection",
  component: ConnectionPanel,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div style={{ width: "600px", minHeight: "500px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof ConnectionPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Default idle state
 */
export const Default: Story = {
  args: {
    state: "idle",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
};

/**
 * With code entered
 */
export const WithCode: Story = {
  args: {
    state: "idle",
    code: "ABC234XYZ",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
};

/**
 * Invalid code (too short)
 */
export const InvalidCode: Story = {
  args: {
    state: "idle",
    code: "ABC",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
};

/**
 * Connecting state with spinner
 */
export const Connecting: Story = {
  args: {
    state: "connecting",
    onCancel: fn(),
  },
};

/**
 * Error state with message
 */
export const Error: Story = {
  args: {
    state: "error",
    code: "ABC234XYZ",
    errorMessage: "無効なコードです",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
};

/**
 * Error - Room not found
 */
export const ErrorRoomNotFound: Story = {
  args: {
    state: "error",
    code: "XYZ999HJK",
    errorMessage: "ルームが見つかりません",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
};

/**
 * Error - Room full
 */
export const ErrorRoomFull: Story = {
  args: {
    state: "error",
    code: "ABC234XYZ",
    errorMessage: "ルームが満員です",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
};

/**
 * English labels
 */
export const English: Story = {
  args: {
    state: "idle",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    title: "jamuru",
    welcomeTitle: "Welcome to jamuru",
    welcomeSubtitle: "Start a low-latency, high-quality audio session",
    createRoomText: "Create Room",
    orText: "or",
    codeLabel: "Invitation Code",
    codePlaceholder: "Enter code",
    joinText: "Join",
  },
};

/**
 * English - Connecting
 */
export const EnglishConnecting: Story = {
  args: {
    state: "connecting",
    onCancel: fn(),
    title: "jamuru",
    connectingText: "Connecting...",
    cancelText: "Cancel",
  },
};

/**
 * English - Error
 */
export const EnglishError: Story = {
  args: {
    state: "error",
    code: "ABC234XYZ",
    errorMessage: "Invalid code",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    title: "jamuru",
    welcomeTitle: "Welcome to jamuru",
    welcomeSubtitle: "Start a low-latency, high-quality audio session",
    createRoomText: "Create Room",
    orText: "or",
    codeLabel: "Invitation Code",
    codePlaceholder: "Enter code",
    joinText: "Join",
  },
};

/**
 * Connecting, with the server it is dialing shown below the spinner - a
 * leftover dev/test URL is visible even before the connection fails.
 */
export const ConnectingToServer: Story = {
  args: {
    state: "connecting",
    onCancel: fn(),
    serverUrl: "signaling.example.com",
  },
};

/**
 * Server-level error: the app could not reach the signaling server at all.
 * Shows the URL, the raw error, and a Retry button; Create/Join are
 * disabled with a reason instead of silently doing nothing.
 */
export const ServerError: Story = {
  args: {
    state: "error",
    errorKind: "server",
    errorMessage: "接続できませんでした: しばらく待ってから再試行してください",
    rawErrorMessage: "Signaling error: Connect failed: HTTP error: 530 <unknown status code>",
    serverUrl: "signaling.example.com",
    connected: false,
    notConnectedReason: "まだサーバーに接続できていません",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    onRetry: fn(),
  },
};

/**
 * Server-level error - English
 */
export const EnglishServerError: Story = {
  args: {
    state: "error",
    errorKind: "server",
    errorMessage: "Connection Failed: Please wait a moment and try again.",
    rawErrorMessage: "Signaling error: Connect failed: HTTP error: 530 <unknown status code>",
    serverUrl: "signaling.example.com",
    connected: false,
    notConnectedReason: "Not connected to the server yet",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    onRetry: fn(),
    title: "jamuru",
    welcomeTitle: "Welcome to jamuru",
    welcomeSubtitle: "Start a low-latency, high-quality audio session",
    createRoomText: "Create Room",
    orText: "or",
    codeLabel: "Invitation Code",
    codePlaceholder: "Enter code",
    joinText: "Join",
  },
};

/**
 * Without settings button
 */
export const NoSettings: Story = {
  args: {
    state: "idle",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
  },
};

/**
 * With test room link in footer
 */
export const WithTestRoom: Story = {
  args: {
    state: "idle",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    testRoomCode: "ABC234XYZ",
  },
};

/**
 * With test room link - English
 */
export const WithTestRoomEnglish: Story = {
  args: {
    state: "idle",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    testRoomCode: "ABC234XYZ",
    title: "jamuru",
    welcomeTitle: "Welcome to jamuru",
    welcomeSubtitle: "Start a low-latency, high-quality audio session",
    createRoomText: "Create Room",
    orText: "or",
    codeLabel: "Invitation Code",
    codePlaceholder: "Enter code",
    joinText: "Join",
    testRoomTitle: "Test Room",
    testRoomDescription: "Connect to a standing room for testing",
  },
};

/**
 * With custom test room code
 */
export const WithCustomTestRoomCode: Story = {
  args: {
    state: "idle",
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
    testRoomCode: "DEMO99",
    testRoomTitle: "デモルーム",
    testRoomDescription: "デモ用のルームに接続します",
  },
};

/**
 * Interactive demo with state management
 */
export const Interactive: Story = {
  args: {
    onCreateRoom: fn(),
    onJoinRoom: fn(),
    onOpenSettings: fn(),
  },
  render: () => {
    const [state, setState] = useState<"idle" | "connecting" | "error">("idle");
    const [code, setCode] = useState("");
    const [error, setError] = useState<string>();

    const handleCreateRoom = () => {
      setState("connecting");
      // Simulate connection
      setTimeout(() => {
        setState("idle");
        alert("Room created! (simulated)");
      }, 2000);
    };

    const handleJoinRoom = (inputCode: string) => {
      setState("connecting");
      // Simulate connection with possible error
      setTimeout(() => {
        if (inputCode === "ERROR1") {
          setState("error");
          setError("Invalid code");
        } else if (inputCode === "ERROR2") {
          setState("error");
          setError("Room not found");
        } else {
          setState("idle");
          alert(`Joined room ${inputCode}! (simulated)`);
        }
      }, 1500);
    };

    const handleCancel = () => {
      setState("idle");
      setError(undefined);
    };

    return (
      <div style={{ width: "600px", height: "500px" }}>
        <ConnectionPanel
          state={state}
          code={code}
          errorMessage={error}
          onCreateRoom={handleCreateRoom}
          onJoinRoom={handleJoinRoom}
          onCodeChange={setCode}
          onCancel={handleCancel}
          onOpenSettings={() => alert("Settings clicked!")}
        />
      </div>
    );
  },
};
