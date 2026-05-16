import { Worker } from 'bullmq';
import { sendEmail } from '../handlers/send';

const worker = new Worker('email-notifications', sendEmail);
