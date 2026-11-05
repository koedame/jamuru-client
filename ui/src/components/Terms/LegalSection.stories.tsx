import type { Meta, StoryObj } from "@storybook/react-vite";
import { LegalSection } from "./LegalSection";

const meta = {
  title: "Components/Terms/LegalSection",
  component: LegalSection,
  parameters: { layout: "centered" },
  decorators: [
    (Story) => (
      <div style={{ width: "450px", background: "var(--color-bg-primary)", padding: "16px" }}>
        <Story />
      </div>
    ),
  ],
  tags: ["autodocs"],
  args: {
    onOpenTerms: () => {},
    onOpenLicense: () => {},
    onOpenPrivacy: () => {},
    onOpenAnnouncements: () => {},
  },
} satisfies Meta<typeof LegalSection>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
