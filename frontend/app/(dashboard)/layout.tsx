import { DashboardRouteLayout } from "@/components/layout/dashboard-route-layout";

export default function DashboardGroupLayout({ children }: { children: React.ReactNode }) {
  return <DashboardRouteLayout>{children}</DashboardRouteLayout>;
}
