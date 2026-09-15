-- A named subnet group stores canonical CSV in the existing cidr column.
-- Existing IDs and single-CIDR records remain valid; groups may share CIDRs.
DROP INDEX IF EXISTS idx_subnets_cidr;
