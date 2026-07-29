import { redirect } from "next/navigation";
import { DASHBOARD_ACCOUNT_CLIENT_PATH } from "@/lib/dashboard-nav";

/** Legacy Account → Client rentals → Client monitor (User tab). */
export default function Page() {
  redirect(`${DASHBOARD_ACCOUNT_CLIENT_PATH}?tab=user`);
}
