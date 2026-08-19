import { redirect } from "next/navigation";

export default function Page() {
  // The former Token Market registry is retired. Keep the URL as a safe
  // compatibility redirect so bookmarks cannot expose a stale control plane.
  redirect("/share-market/");
}
