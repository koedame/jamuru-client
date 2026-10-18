import type { Meta, StoryObj } from "@storybook/react-vite";
import { EntryNameDialog } from "./EntryNameDialog";

const meta = {
  title: "Components/EntryNameDialog",
  component: EntryNameDialog,
  parameters: {
    layout: "fullscreen",
  },
  decorators: [
    (Story) => (
      <div style={{ position: "relative", width: "600px", height: "400px", background: "var(--color-bg-primary)" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  argTypes: {
    open: { control: "boolean" },
    pending: { control: "boolean" },
  },
} satisfies Meta<typeof EntryNameDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The name used last time is in the field. */
export const Default: Story = {
  args: {
    open: true,
    initialName: "Aki",
    confirmLabel: "Create room",
    onConfirm: () => {},
    onCancel: () => {},
  },
};

/** Nothing has been entered yet: the guest name is offered. */
export const Guest: Story = {
  args: {
    open: true,
    initialName: "Guest",
    confirmLabel: "Join",
    onConfirm: () => {},
    onCancel: () => {},
  },
};

/** An empty name cannot be confirmed. */
export const Empty: Story = {
  args: {
    open: true,
    initialName: "",
    confirmLabel: "Join",
    onConfirm: () => {},
    onCancel: () => {},
  },
};

/** The name is being saved. */
export const Pending: Story = {
  args: {
    open: true,
    initialName: "Aki",
    confirmLabel: "Join",
    pending: true,
    onConfirm: () => {},
    onCancel: () => {},
  },
};

/** The name could not be saved. */
export const SaveFailed: Story = {
  args: {
    open: true,
    initialName: "Aki",
    confirmLabel: "Join",
    error: "Could not save the name",
    onConfirm: () => {},
    onCancel: () => {},
  },
};

export const Closed: Story = {
  args: {
    open: false,
    initialName: "Aki",
    confirmLabel: "Join",
    onConfirm: () => {},
    onCancel: () => {},
  },
};
