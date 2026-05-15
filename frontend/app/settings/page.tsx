import { AppShell } from "@/components/layout/app-shell";
import { SettingsPage } from "@/components/settings/settings-page";

export default function Page() {
  return (
    <AppShell active="settings">
      <SettingsPage />
    </AppShell>
  );
}
