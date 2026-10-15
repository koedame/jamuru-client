// Main component
export { SettingsPanel, type SettingsPanelProps, type SettingsTabId } from "./SettingsPanel";

// Adapter for Tauri integration
export { SettingsPanelAdapter, type SettingsPanelAdapterProps } from "./SettingsPanelAdapter";

// Sub components
export { VerticalTabs, type VerticalTabsProps, type Tab } from "./VerticalTabs";
export { FormField, type FormFieldProps } from "./FormField";
export { Select, type SelectProps, type SelectOption } from "./Select";
export { Input, type InputProps } from "./Input";

// Tab contents
export {
  GeneralTab,
  DevicesTab,
  PrivacyTab,
  DiagnosticsTab,
  AboutTab,
  type GeneralTabProps,
  type DevicesTabProps,
  type PrivacyTabProps,
  type DiagnosticsTabProps,
  type AboutTabProps,
  type Language,
  type DeviceInfo,
} from "./tabs";
