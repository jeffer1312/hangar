import { useState } from 'react';
import { BlockHead, Box, ConfigRow, Disclosure, Muted, ServerConfigPage } from '../../src/features/config/ServerConfigParts';
import { useServerConfig } from '../../src/features/config/serverConfig';
import * as m from '../../src/paraglide/messages';

export default function Jev() {
  const cfg = useServerConfig();
  const [advanced, setAdvanced] = useState(false);
  return (
    <ServerConfigPage title={m.jev_title()} cfg={cfg}>
      <Muted>{m.jev_intro()}</Muted>
      <BlockHead title={m.jev_browser_title()} help={m.jev_browser_help()} />
      <Box>
        <ConfigRow cfg={cfg} k="jev_api_key" />
        <ConfigRow cfg={cfg} k="jev_padrao" />
      </Box>
      <Disclosure open={advanced} label={m.jev_advanced()} onChange={setAdvanced} />
      {advanced ? (
        <>
          <Box>
            <ConfigRow cfg={cfg} k="jev_endpoint" />
            <ConfigRow cfg={cfg} k="jev_model" />
          </Box>
          <BlockHead title={m.jev_text_title()} help={m.jev_text_help()} />
          <Box>
            {['jev_texto_base_url', 'jev_texto_api_key', 'jev_texto_modelo', 'jev_texto_cmd'].map((k) => <ConfigRow key={k} cfg={cfg} k={k} />)}
          </Box>
        </>
      ) : null}
    </ServerConfigPage>
  );
}
