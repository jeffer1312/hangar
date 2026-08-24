import * as React from 'react';
export const MultiTextInput = (props: any) =>
  React.createElement('textarea', {
    'data-testid': 'editor-input',
    value: props.value,
    onChange: (e: any) => props.onChangeText(e.target.value),
    placeholder: props.placeholder,
  });
