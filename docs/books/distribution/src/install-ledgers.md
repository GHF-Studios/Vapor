# Install Ledgers

The local service records install operations so it can explain which artifact
versions form a Composition and how each artifact was acquired. The authored
Packagepack is the composition root; the install record captures the resolved
dependency graph, source provenance, and local realization.

The Library may contain content that is not installed or not used by the
selected Composition. Subscriptions, downloads, cached artifacts, installed
artifacts, and Composition membership must remain distinguishable.

Vapor-generated indexes, locks, receipts, and verification records are
service-owned state. Do not edit them manually or copy them between unrelated
app roots. Use update or repair operations to reconcile state. The user's
source workspace is separate and should be protected by normal source control.
