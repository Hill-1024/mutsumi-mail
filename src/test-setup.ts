import { configure } from '@testing-library/react';

// Query notifications and router transitions may wait behind parallel test workers on CI.
// Keep the assertions asynchronous without tying them to a one-second machine-speed limit.
configure({ asyncUtilTimeout: 5_000 });
