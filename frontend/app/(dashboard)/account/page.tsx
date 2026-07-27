import { redirect } from "next/navigation";
import { DASHBOARD_ACCOUNT_PAYMENTS_PATH } from "@/lib/dashboard-nav";

export default function Page() {
  redirect(DASHBOARD_ACCOUNT_PAYMENTS_PATH);
}
