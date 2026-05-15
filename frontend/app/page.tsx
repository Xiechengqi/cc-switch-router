import { DashboardPage } from "@/components/dashboard/dashboard-page";
import { AppShell } from "@/components/layout/app-shell";

export default function Page() {
  return (
    <AppShell active="dashboard">
      <DashboardPage />
    </AppShell>
  );
}
