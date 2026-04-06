import { Button, Stack, Text } from '@mantine/core';

type GlobalCapabilitiesPanelProps = {
  onOpenRepoFragment: () => void;
  onOpenChangesetSchema: () => void;
  onOpenApplyChangeset: () => void;
};

export function GlobalCapabilitiesPanel(props: GlobalCapabilitiesPanelProps) {
  const { onOpenRepoFragment, onOpenChangesetSchema, onOpenApplyChangeset } = props;

  return (
    <Stack>
      <Text size="sm">Choose a workflow-global capability to configure.</Text>
      <Button variant="light" onClick={onOpenRepoFragment}>Configure repo fragment</Button>
      <Button variant="light" onClick={onOpenChangesetSchema}>Patch changeset schema</Button>
      <Button variant="light" onClick={onOpenApplyChangeset}>Manually apply changeset</Button>
    </Stack>
  );
}
