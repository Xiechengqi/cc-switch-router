// Empty, buyer-scoped personal rentals for existing browser contract fixtures.
// These mocks do not open a browser or perform any operation by themselves.
export function emptyClientRentalsPage(requestUrl) {
  const url = new URL(requestUrl);
  return { items: [], view: url.searchParams.get("view") || "current", total: 0,
    page: 1, pageSize: 5, totalPages: 1, focusedId: null, serverNow: new Date().toISOString() };
}
