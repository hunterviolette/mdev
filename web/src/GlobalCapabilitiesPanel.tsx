import { Badge, Button, Group, Stack, Text } from '@mantine/core';

type GlobalCapabilitiesPanelProps = {
  onOpenInference: () => void;
  onOpenRepoFragment: () => void;
  onOpenChangesetSchema: () => void;
  onOpenApplyChangeset: () => void;
  repoContextArmed: boolean;
  changesetSchemaArmed: boolean;
};

export function GlobalCapabilitiesPanel(props: GlobalCapabilitiesPanelProps) {
  const {
    onOpenInference,
    onOpenRepoFragment,
    onOpenChangesetSchema,
    onOpenApplyChangeset,
    repoContextArmed,
    changesetSchemaArmed,
  } = props;

  return (
    <Stack>
      <Text size="sm">Choose a workflow-global capability to configure.</Text>
      <Button variant="light" onClick={onOpenInference}>Configure inference defaults</Button>
      <Group justify="space-between" gap="sm" wrap="nowrap">
        <Button variant="light" onClick={onOpenRepoFragment}>Configure repo fragment</Button>
        <Badge color={repoContextArmed ? 'green' : 'gray'} variant="light">{repoContextArmed ? 'Armed' : 'Not armed'}</Badge>
      </Group>
      <Group justify="space-between" gap="sm" wrap="nowrap">
        <Button variant="light" onClick={onOpenChangesetSchema}>Patch changeset schema</Button>
        <Badge color={changesetSchemaArmed ? 'green' : 'gray'} variant="light">{changesetSchemaArmed ? 'Armed' : 'Not armed'}</Badge>
      </Group>
      <Button variant="light" onClick={onOpenApplyChangeset}>Manually apply changeset</Button>
    </Stack>
  );
}
